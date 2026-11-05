// SPDX-License-Identifier: CC0-1.0

//! WHO may reach Control Catalog's operations, driven end to end over the real wire path
//! (a restJson1 request through the multi-service router into the mounted service), so
//! what is asserted is what a client holding those credentials sees.
//!
//! THE RULE UNDER TEST: `AccessDeniedException` is a service-common error of Control
//! Catalog (`smithy_query(query="digest")` lists it under `common_errors`), so EVERY one
//! of the six operations evaluates the identity-based IAM policies in force on the
//! principal its credentials name against `controlcatalog:<OperationName>` BEFORE it reads
//! the catalog — a request no statement allows (IAM's default deny), or one an explicit
//! `Deny` covers, is answered HTTP 403 `AccessDeniedException` and reports nothing, while a
//! request a matching `Allow` covers reaches the operation's own answer. The one operation
//! that addresses a resource is judged against the control ARN the request CARRIED, and
//! names it in the refusal; every other is judged against the resource IAM writes for a
//! request that addresses none, and names none.
//!
//! The role, the user, their policies and the session live in the IAM and STS stores of the
//! SAME shared engine — the stores those services' own handlers write — so the documents
//! under test are the ones `iam:PutRolePolicy` stores and the principal a refusal blames is
//! the one `sts:GetCallerIdentity` answers for the key the request signed with.
//!
//! The tables span what the rule generalises over: the operation (all six), the document in
//! force (none at all, another action of this service, another service's actions, an
//! explicit deny beside an allow, an allow of exactly the action, of the action in another
//! case, of the whole service and of everything), the principal (an assumed-role session,
//! an IAM user's long-term access key and the account's own published credential), the
//! resource an allow is scoped to (every resource, this partition's controls, one control,
//! another control) and the ARN FORM the request names a control with (both forms GetControl
//! documents).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_controlcatalog_controlcatalog::state::ComAmazonawsControlcatalogControlcatalogState as Catalog;
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam;
use service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts;
use smithy_server_runtime::MultiServiceRouter;

use crate::control_catalog::error_code;
use crate::validation_messages::message_of;

/// The Region every call below is signed for. The catalog itself is global — its ARNs
/// carry no Region — so this only names the credential scope the router routes on.
const REGION: &str = "us-east-1";

/// The access key id of the account's own published credential: a key neither STS nor IAM
/// holds a record for, which is the position AWS account root credentials are in.
const ACCOUNT_KEY_ID: &str = "AKIDTEST";

/// The access key id of the assumed-role session the role rows sign with.
const SESSION_KEY_ID: &str = "ASIAEXAMPLESESSION01";

/// The access key id of the IAM user's long-term key the user rows sign with.
const USER_KEY_ID: &str = "AKIAEXAMPLEUSERKEY01";

/// The role the session is assumed from, the session's own name, and the IAM user.
const ROLE: &str = "catalog-reader";
const SESSION: &str = "catalog-session";
const USER_NAME: &str = "catalog-user";

/// The name of the inline policy the role and the user carry.
const POLICY_NAME: &str = "inline";

/// How long the seeded session lasts: an hour, STS's own default.
const SESSION_SECONDS: i32 = 3_600;

/// A router serving Control Catalog beside the IAM and STS services, over ONE shared
/// engine, so the policies and the session the cases seed are the ones those services hold.
fn router_with_identity() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_controlcatalog_controlcatalog::EmulatedService::new(Arc::clone(
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

/// Call one Control Catalog route signed with `access_key_id` — what names the principal
/// the request speaks for.
async fn call_as(
    router: &MultiServiceRouter,
    access_key_id: &str,
    route: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("http://localhost{route}"))
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential={access_key_id}/20260101/{REGION}/controlcatalog/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("a valid request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = serde_json::from_slice::<Value>(response.body()).unwrap_or(Value::Null);
    (status, parsed)
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
            role_id: "AROAEXAMPLECATALOGROLE".to_string(),
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
    let store = sts::service_state(&**engine).expect("the STS store");
    store
        .lock()
        .insert(sts::Session {
            access_key_id: SESSION_KEY_ID.to_string(),
            created_at: now,
            expires_at: now + Duration::from_secs(SESSION_SECONDS as u64),
            duration_seconds: SESSION_SECONDS,
            identity: sts::Identity {
                user_id: sts::role_id(&role_arn),
                account: account.to_string(),
                arn: session_arn(),
            },
            kind: sts::SessionKind::AssumedRole,
            source_identity: None,
            tags: Vec::new(),
            transitive_tag_keys: Vec::new(),
            packed_policy_size: None,
            policies: sts::SessionPolicies::default(),
            external_id: None,
            role_arn: Some(role_arn),
            revoked: false,
        })
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
            user_id: "AIDAEXAMPLECATALOGUSER".to_string(),
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
                    { "Effect": "Deny", "Action": "controlcatalog:*", "Resource": "*" },
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
            Some(allow(json!("controlcatalog:*"))),
            None,
        ),
        ("an allow of every action", Some(allow(json!("*"))), None),
    ]
}

/// A control ARN the seeded catalog publishes, which `GetControl` answers 200 for.
fn published_control_arn() -> String {
    Catalog::default()
        .controls
        .keys()
        .next()
        .expect("the catalog publishes controls")
        .clone()
}

/// The six operations this suite holds to the rule, with the route, the request each is
/// given, the resource clause its refusal carries, and the response member a served call
/// answers with.
///
/// The request has to be one the operation would otherwise SERVE, so an authorized row
/// reaches a 200 rather than another refusal.
fn authorized_operations() -> Vec<(&'static str, &'static str, Value, String, &'static str)> {
    let control = published_control_arn();
    vec![
        (
            "GetControl",
            "/get-control",
            json!({ "ControlArn": control }),
            format!(" on resource: {control}"),
            "Arn",
        ),
        (
            "ListControls",
            "/list-controls",
            json!({}),
            String::new(),
            "Controls",
        ),
        (
            "ListControlMappings",
            "/list-control-mappings",
            json!({}),
            String::new(),
            "ControlMappings",
        ),
        (
            "ListDomains",
            "/domains",
            json!({}),
            String::new(),
            "Domains",
        ),
        (
            "ListObjectives",
            "/objectives",
            json!({}),
            String::new(),
            "Objectives",
        ),
        (
            "ListCommonControls",
            "/common-controls",
            json!({}),
            String::new(),
            "CommonControls",
        ),
    ]
}

/// Require the refusal `answer` to be IAM's, naming `principal`, `action` and `clause`.
fn assert_refusal(case: &str, status: u16, answer: &Value, expected_message: &str) {
    assert_eq!(
        status, 403,
        "case {case:?}: expected HTTP 403, got {status} {answer}"
    );
    assert_eq!(
        error_code(answer),
        "AccessDeniedException",
        "case {case:?}: expected an AccessDeniedException, got {answer}"
    );
    let message = message_of(answer)
        .unwrap_or_else(|| panic!("case {case:?}: the refusal carries no message: {answer}"));
    assert_eq!(
        message, expected_message,
        "case {case:?}: expected IAM's wording, got {message:?}"
    );
}

/// THE RULE: every operation is judged against its OWN `controlcatalog:<OperationName>`
/// action before it reads the catalog; a refusal is HTTP 403 `AccessDeniedException` naming
/// the principal, the action, the resource the request addressed and which of IAM's rules
/// refused it, and an allowed call reaches the operation's own answer.
#[tokio::test]
async fn every_operation_is_authorized_against_its_own_action() {
    for (operation, route, request, clause, served_member) in authorized_operations() {
        let action = format!("controlcatalog:{operation}");
        // Another action of this service, which a row grants instead of the one under test.
        let other = authorized_operations()
            .into_iter()
            .map(|(name, ..)| format!("controlcatalog:{name}"))
            .find(|candidate| *candidate != action)
            .expect("this service has more than one operation");

        for (row, document, refused) in documents_in_force(&action, &other) {
            let case = format!("{operation} with {row}");
            // A fresh store, role and session per row: no row can pass on another's write.
            let (router, engine) = router_with_identity();
            given_session(&engine);
            given_role_policy(&engine, document.as_ref());

            let (status, answer) = call_as(&router, SESSION_KEY_ID, route, &request).await;
            match refused {
                Some(refusal) => {
                    assert_refusal(
                        &case,
                        status,
                        &answer,
                        &format!(
                            "User: {} is not authorized to perform: {action}{clause} {}",
                            session_arn(),
                            refusal.reason(&action)
                        ),
                    );
                    // The refusal carries no part of the operation's own answer: an
                    // unauthorized caller is told nothing about the catalog.
                    assert!(
                        answer.get(served_member).is_none(),
                        "case {case:?}: a refused call must report no {served_member}, got \
                         {answer}"
                    );
                }
                None => {
                    assert_eq!(
                        status, 200,
                        "case {case:?}: an allowed request must be served, got {status} {answer}"
                    );
                    assert!(
                        answer.get(served_member).is_some(),
                        "case {case:?}: a served call must report {served_member}, got {answer}"
                    );
                }
            }
        }
    }
}

/// RULE: an IAM user's long-term access key speaks for THAT user, so the decision is taken
/// on the user's own policies and the refusal names the user's ARN — the same rule the
/// session rows exercise, under the other principal form.
#[tokio::test]
async fn an_iam_user_key_is_judged_on_the_users_own_policies() {
    for (operation, route, request, clause, served_member) in authorized_operations() {
        let action = format!("controlcatalog:{operation}");
        // (case, the document in force, the refusal it meets)
        let rows: Vec<(&str, Option<Value>, Option<Refusal>)> = vec![
            ("no policy at all", None, Some(Refusal::DefaultDeny)),
            (
                "an explicit deny of this service",
                Some(json!({
                    "Statement": [
                        { "Effect": "Deny", "Action": "controlcatalog:*", "Resource": "*" },
                    ],
                })),
                Some(Refusal::ExplicitDeny),
            ),
            (
                "an allow of every action of this service",
                Some(allow(json!("controlcatalog:*"))),
                None,
            ),
        ];

        for (row, document, refused) in rows {
            let case = format!("{operation} as an IAM user with {row}");
            let (router, engine) = router_with_identity();
            given_user_key(&engine);
            given_user_policy(&engine, document.as_ref());

            let (status, answer) = call_as(&router, USER_KEY_ID, route, &request).await;
            match refused {
                Some(refusal) => assert_refusal(
                    &case,
                    status,
                    &answer,
                    &format!(
                        "User: {} is not authorized to perform: {action}{clause} {}",
                        user_arn(),
                        refusal.reason(&action)
                    ),
                ),
                None => {
                    assert_eq!(
                        status, 200,
                        "case {case:?}: an allowed request must be served, got {status} {answer}"
                    );
                    assert!(
                        answer.get(served_member).is_some(),
                        "case {case:?}: a served call must report {served_member}, got {answer}"
                    );
                }
            }
        }
    }
}

/// RULE: the account's own published credential — a key neither STS nor IAM holds a record
/// for — is not a principal the IAM store holds policies for, so it is served whatever the
/// role's policies say.
#[tokio::test]
async fn the_accounts_own_credential_is_served() {
    for (operation, route, request, _clause, served_member) in authorized_operations() {
        let (router, engine) = router_with_identity();
        given_session(&engine);
        // The role carries nothing at all, which is what refuses the SESSION above.
        given_role_policy(&engine, None);

        let (status, answer) = call_as(&router, ACCOUNT_KEY_ID, route, &request).await;
        assert_eq!(
            status, 200,
            "case {operation:?}: the account's own credential must be served, got {status} \
             {answer}"
        );
        assert!(
            answer.get(served_member).is_some(),
            "case {operation:?}: a served call must report {served_member}, got {answer}"
        );
    }
}

/// RULE: `GetControl` is judged against the control ARN the request NAMES, so an allow
/// scoped to one control covers that control and no other — and the ARN under judgement is
/// the one the request carried, in whichever of the two forms the operation accepts, not
/// the catalog ARN it would have answered with.
#[tokio::test]
async fn get_control_is_judged_against_the_control_the_request_names() {
    let catalog = Catalog::default();
    let control = catalog
        .controls
        .values()
        .find(|control| control.control_tower_identifier.is_some())
        .expect("the catalog publishes a control that is also a Control Tower control");
    let catalog_arn = control.arn.clone();
    // The other ARN form `GetControl` documents for the SAME control.
    let tower_arn = format!(
        "arn:aws:controltower:us-east-1::control/{}",
        control
            .control_tower_identifier
            .as_deref()
            .expect("the row's control carries a Control Tower identifier")
    );
    let other_arn = catalog
        .controls
        .keys()
        .find(|arn| **arn != catalog_arn)
        .expect("the catalog publishes more than one control")
        .clone();

    // (case, the ARN the request names, the Resource the allow is scoped to, allowed).
    // IAM compares resource ARNs literally, with `*` and `?` as the only wildcards, so an
    // allow scoped to one spelling of a control does not cover the other spelling of the
    // same control — which is what makes the ARN AS SENT the thing under judgement.
    let rows: Vec<(String, &String, Value, bool)> = vec![
        (
            "the catalog ARN under an allow of every resource".to_string(),
            &catalog_arn,
            json!("*"),
            true,
        ),
        (
            "the catalog ARN under an allow of exactly it".to_string(),
            &catalog_arn,
            json!(catalog_arn.clone()),
            true,
        ),
        (
            "the catalog ARN under an allow of every catalog control".to_string(),
            &catalog_arn,
            json!("arn:aws:controlcatalog:::control/*"),
            true,
        ),
        (
            "the catalog ARN under an allow of another control only".to_string(),
            &catalog_arn,
            json!(other_arn.clone()),
            false,
        ),
        (
            "the catalog ARN under an allow of every Control Tower control".to_string(),
            &catalog_arn,
            json!("arn:aws:controltower:*::control/*"),
            false,
        ),
        (
            "the Control Tower ARN under an allow of every resource".to_string(),
            &tower_arn,
            json!("*"),
            true,
        ),
        (
            "the Control Tower ARN under an allow of every Control Tower control".to_string(),
            &tower_arn,
            json!("arn:aws:controltower:*::control/*"),
            true,
        ),
        (
            "the Control Tower ARN under an allow of the same control's catalog ARN"
                .to_string(),
            &tower_arn,
            json!(catalog_arn.clone()),
            false,
        ),
        (
            "the Control Tower ARN under an allow of every catalog control".to_string(),
            &tower_arn,
            json!("arn:aws:controlcatalog:::control/*"),
            false,
        ),
    ];

    for (case, requested, resource, allowed) in rows {
        let (router, engine) = router_with_identity();
        given_session(&engine);
        given_role_policy(
            &engine,
            Some(&json!({
                "Statement": [{
                    "Effect": "Allow",
                    "Action": "controlcatalog:GetControl",
                    "Resource": resource,
                }],
            })),
        );

        let (status, answer) = call_as(
            &router,
            SESSION_KEY_ID,
            "/get-control",
            &json!({ "ControlArn": requested }),
        )
        .await;
        if allowed {
            assert_eq!(
                status, 200,
                "case {case:?}: an allowed request must be served, got {status} {answer}"
            );
            // Both accepted forms name one control, so a served call answers with the
            // catalog ARN whichever form was asked with.
            assert_eq!(
                answer.get("Arn").and_then(Value::as_str),
                Some(catalog_arn.as_str()),
                "case {case:?}: expected the catalog ARN, got {answer}"
            );
        } else {
            let action = "controlcatalog:GetControl";
            assert_refusal(
                &case,
                status,
                &answer,
                &format!(
                    "User: {} is not authorized to perform: {action} on resource: {requested} \
                     because no identity-based policy allows the {action} action",
                    session_arn()
                ),
            );
            assert!(
                answer.get("Name").is_none(),
                "case {case:?}: a refused call must report no control, got {answer}"
            );
        }
    }
}
