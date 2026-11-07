// SPDX-License-Identifier: CC0-1.0

//! WHO may reach the Agent Registry discovery reads, driven end to end over the real wire
//! path (a restJson1 request through the multi-service router into the mounted service),
//! so what is asserted is what a client holding those credentials sees.
//!
//! THE RULE UNDER TEST: `AccessDeniedException` is a service-common error of this service
//! (`smithy_query(query="digest")` lists it under `common_errors`), so EVERY one of the
//! three discovery reads evaluates the identity-based IAM policies in force on the
//! principal its credentials name against `agent-registry:<OperationName>` BEFORE it
//! resolves the registry the request names — a request no statement allows (IAM's default
//! deny), or one an explicit `Deny` covers, is answered HTTP 403 `AccessDeniedException`
//! and reports nothing, while a request a matching `Allow` covers reaches the operation's
//! own answer. The decision is taken before the store is read, so it does not depend on
//! whether the registry exists. Two things are read per operation, as recorded AWS
//! behaviour shows them: WHICH RESOURCE the decision is taken against — the list and the
//! search resolve the identifier to the registry's ARN, whichever of the two forms
//! `RegistryIdentifier` admits the caller spelled, while the batch read decides only for
//! an entry that names its registry by ARN — and HOW a refusal is worded: IAM's own
//! sentence for the two registry-scoped reads, and the batch read's own fixed sentence for
//! the batch read.
//!
//! The role, the user, their policies and the session live in the IAM and STS stores of
//! the SAME shared engine — the stores those services' own handlers write — so the
//! documents under test are the ones `iam:PutRolePolicy` stores and the principal a
//! refusal blames is the one `sts:GetCallerIdentity` answers for the key the request
//! signed with.
//!
//! The tables span what the rule generalises over: the operation (all three), the document
//! in force (none at all, another action of this service, another service's actions, an
//! explicit deny beside an allow, an allow of exactly the action, of the action in another
//! case, of the whole service and of everything), the principal (an assumed-role session,
//! an IAM user's long-term access key and the account's own published credential), the
//! resource an allow is scoped to (every resource, every registry of the account, this
//! registry, another registry), whether the registry EXISTS, and the identifier form the
//! request names it with.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_agentregistry_agentregistry::state::{
    ComAmazonawsAgentregistryAgentregistryState, RegistryRecord,
};
use service_com_amazonaws_agentregistry_agentregistry::EmulatedService;
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam;
use service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts;
use smithy_server_runtime::MultiServiceRouter;

use crate::discovery_read_path::{encode_label, list_path};

/// The Region every call below is signed for, and so the Region of the registry ARN each
/// decision is made against.
const REGION: &str = "us-east-1";

/// The registry the seeded store holds, and one it does not: the rule is about the
/// authorization decision, which is taken before either is looked up.
const REGISTRY: &str = "abc123def456";
const ABSENT_REGISTRY: &str = "zzz999999999";

/// A record id satisfying `RecordIdentifier` (12 alphanumerics), so a batch request is
/// refused on its registry and nothing else.
const RECORD_ID: &str = "rec000000001";

/// The access key id of the account's own published credential: a key neither STS nor IAM
/// holds a record for, which is the position AWS account root credentials are in.
const ACCOUNT_KEY_ID: &str = "AKIDTEST";

/// The access key id of the assumed-role session the role rows sign with.
const SESSION_KEY_ID: &str = "ASIAEXAMPLESESSION01";

/// The access key id of the IAM user's long-term key the user rows sign with.
const USER_KEY_ID: &str = "AKIAEXAMPLEUSERKEY01";

/// The role the session is assumed from, the session's own name, and the IAM user.
const ROLE: &str = "registry-reader";
const SESSION: &str = "registry-session";
const USER_NAME: &str = "registry-user";

/// The name of the inline policy the role and the user carry.
const POLICY_NAME: &str = "inline";

/// How long the seeded session lasts: an hour, STS's own default.
const SESSION_SECONDS: i32 = 3_600;

/// A router serving Agent Registry beside the IAM and STS services, over ONE shared
/// engine, so the policies and the session the cases seed are the ones those services
/// hold. The store carries [`REGISTRY`] and one discoverable record in it, so an allowed
/// call has something to answer with.
fn router_with_identity() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let state = engine.state::<ComAmazonawsAgentregistryAgentregistryState>();
    state.lock().put_record(
        REGISTRY,
        RegistryRecord::new(
            RECORD_ID,
            "payments-mcp",
            "MCP",
            "1.0.0",
            "APPROVED",
            ::aws_smithy_types::DateTime::from_secs(1_767_225_600),
        ),
    );
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

/// Call one Agent Registry route signed with `access_key_id` — what names the principal
/// the request speaks for.
async fn call_as(
    router: &MultiServiceRouter,
    access_key_id: &str,
    route: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!(
            "https://agent-registry.{REGION}.amazonaws.com{route}"
        ))
        .header("host", format!("agent-registry.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential={access_key_id}/20260101/{REGION}/agent-registry/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a valid request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The error code a refusal body carries (`__type` on restJson1), without the shape's
/// namespace.
fn error_code(body: &Value) -> String {
    body["__type"]
        .as_str()
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The `message` member of a refusal body.
fn stated(body: &Value) -> String {
    body["message"].as_str().unwrap_or_default().to_string()
}

/// The ARN of a registry of that id, as this service publishes it in the Region the
/// request signed for.
fn registry_arn(registry_id: &str) -> String {
    format!(
        "arn:aws:agent-registry:{REGION}:{}:registry/{registry_id}",
        ::engine::emulator_account_id()
    )
}

/// The registry id inside an identifier of either form.
fn registry_id_of(identifier: &str) -> &str {
    identifier
        .rsplit_once(":registry/")
        .map_or(identifier, |(_, id)| id)
}

/// The sentence the BATCH read words its own refusal with, as recorded: one fixed sentence
/// about the records asked for, carrying no part of the request.
const BATCH_READ_REFUSAL: &str = "Caller is not authorized to read the requested records.";

/// The whole sentence a refusal of `operation` carries for `principal` naming
/// `identifier`, under `refusal`: IAM's own for the two registry-scoped reads, and this
/// service's fixed one for the batch read.
fn refusal_message(
    operation: &str,
    identifier: &str,
    principal: &str,
    refusal: Refusal,
) -> String {
    if operation == BATCH_READ {
        return BATCH_READ_REFUSAL.to_string();
    }
    let action = format!("agent-registry:{operation}");
    format!(
        "User: {principal} is not authorized to perform: {action} on resource: {} {}",
        registry_arn(registry_id_of(identifier)),
        refusal.reason(&action)
    )
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
            role_id: "AROAEXAMPLEREGISTRYROLE".to_string(),
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
            user_id: "AIDAEXAMPLEREGISTRYUSER".to_string(),
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
                    { "Effect": "Deny", "Action": "agent-registry:*", "Resource": "*" },
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
            Some(allow(json!("agent-registry:*"))),
            None,
        ),
        ("an allow of every action", Some(allow(json!("*"))), None),
    ]
}

/// Every discovery read, as the request that reaches it for `registry`: the operation's
/// name, its route, the body naming that registry, and the response member a served call
/// answers with.
fn reads_of(registry: &str) -> [(&'static str, String, Value, &'static str); 3] {
    [
        (
            "ListDiscoverableRegistryRecords",
            list_path(registry),
            json!({}),
            "registryRecords",
        ),
        (
            "SearchDiscoverableRegistryRecords",
            "/discoverable-records-search".to_string(),
            json!({ "searchQuery": "payments", "registryIds": [registry], "maxResults": 10 }),
            "registryRecords",
        ),
        (
            BATCH_READ,
            "/discoverable-records-batch".to_string(),
            json!({ "entries": [{ "registryId": registry, "recordIds": [RECORD_ID] }] }),
            "registryRecords",
        ),
    ]
}

/// The one read whose decision is taken only for an entry naming its registry by ARN.
const BATCH_READ: &str = "BatchGetDiscoverableRegistryRecord";

/// One read's route, request and served member, for a request naming `identifier`.
fn read_of(operation: &str, identifier: &str) -> (String, Value, &'static str) {
    reads_of(identifier)
        .into_iter()
        .find(|(name, ..)| *name == operation)
        .map(|(_, route, request, served)| (route, request, served))
        .expect("the operation is one of this service's reads")
}

/// The identifier form a read takes a decision FOR: the batch read decides only for an
/// entry that names its registry by ARN, so the tables that are about the decision drive it
/// with that form, while the two registry-scoped reads decide for either form and are
/// driven with the bare id.
fn judged_identifier(operation: &str, registry_id: &str) -> String {
    if operation == BATCH_READ {
        registry_arn(registry_id)
    } else {
        registry_id.to_string()
    }
}

/// Require the refusal `answer` to be IAM's, worded exactly as `expected_message`.
fn assert_refusal(case: &str, status: u16, answer: &Value, expected_message: &str) {
    assert_eq!(
        status, 403,
        "case {case:?}: expected HTTP 403, got {status} {answer}"
    );
    let code = error_code(answer);
    assert_eq!(
        code, "AccessDeniedException",
        "case {case:?}: expected an AccessDeniedException, got {code:?}: {answer}"
    );
    let message = stated(answer);
    assert_eq!(
        message, expected_message,
        "case {case:?}: expected IAM's wording, got {message:?}"
    );
}

/// THE RULE: every discovery read is judged against its OWN
/// `agent-registry:<OperationName>` action before the store is read; a refusal is HTTP 403
/// `AccessDeniedException` naming the principal, the action, the resource the request
/// carried and which of IAM's rules refused it, and an allowed call reaches the
/// operation's own answer.
#[tokio::test]
async fn every_discovery_read_is_authorized_against_its_own_action() {
    for (operation, ..) in reads_of(REGISTRY) {
        let action = format!("agent-registry:{operation}");
        // Another action of this service, which a row grants instead of the one under
        // test.
        let other = reads_of(REGISTRY)
            .into_iter()
            .map(|(name, ..)| format!("agent-registry:{name}"))
            .find(|candidate| *candidate != action)
            .expect("this service has more than one operation");
        let identifier = judged_identifier(operation, REGISTRY);
        let (route, request, served_member) = read_of(operation, &identifier);

        for (row, document, refused) in documents_in_force(&action, &other) {
            let case = format!("{operation} with {row}");
            // A fresh store, role and session per row: no row can pass on another's write.
            let (router, engine) = router_with_identity();
            given_session(&engine);
            given_role_policy(&engine, document.as_ref());

            let (status, answer) = call_as(&router, SESSION_KEY_ID, &route, &request).await;
            match refused {
                Some(refusal) => {
                    assert_refusal(
                        &case,
                        status,
                        &answer,
                        &refusal_message(operation, &identifier, &session_arn(), refusal),
                    );
                    // The refusal carries no part of the operation's own answer: an
                    // unauthorized caller is told nothing about the registry.
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

/// RULE: the decision is taken BEFORE the registry is resolved, so a caller no policy
/// allows is refused `AccessDeniedException` whether or not the registry exists — it is
/// never told which — while the same caller, once allowed, meets the registry's own
/// answer: `404 ResourceNotFoundException` for one this service does not hold.
#[tokio::test]
async fn authorization_is_decided_before_the_registry_is_resolved() {
    for (registry, present) in [(REGISTRY, true), (ABSENT_REGISTRY, false)] {
        for (operation, ..) in reads_of(registry) {
            let case = format!(
                "{operation} naming a registry that {}",
                if present { "exists" } else { "does not exist" }
            );
            let identifier = judged_identifier(operation, registry);
            let (route, request, _) = read_of(operation, &identifier);

            // Unauthorized: the same 403 for both, so nothing of the registry's existence
            // reaches the caller.
            let (router, engine) = router_with_identity();
            given_session(&engine);
            given_role_policy(&engine, None);
            let (status, answer) = call_as(&router, SESSION_KEY_ID, &route, &request).await;
            assert_refusal(
                &case,
                status,
                &answer,
                &refusal_message(operation, &identifier, &session_arn(), Refusal::DefaultDeny),
            );

            // Authorized: the operation's own answer, which is where existence is decided.
            let (router, engine) = router_with_identity();
            given_session(&engine);
            given_role_policy(&engine, Some(&allow(json!("agent-registry:*"))));
            let (status, answer) = call_as(&router, SESSION_KEY_ID, &route, &request).await;
            if present {
                assert_eq!(
                    status, 200,
                    "case {case:?}: an allowed request must be served, got {status} {answer}"
                );
            } else {
                assert_eq!(
                    status, 404,
                    "case {case:?}: expected the registry's own 404, got {status} {answer}"
                );
                assert_eq!(
                    error_code(&answer),
                    "ResourceNotFoundException",
                    "case {case:?}: expected ResourceNotFoundException, got {answer}"
                );
                assert_eq!(
                    stated(&answer),
                    format!("Registry not found: {identifier}"),
                    "case {case:?}: expected the not-found sentence, got {answer}"
                );
            }
        }
    }
}

/// RULE: the resource a decision is made against is the one recorded AWS behaviour shows
/// for that read — the two registry-scoped reads resolve the identifier to the registry's
/// ARN, so a statement scoped to that ARN covers BOTH forms the model admits, while the
/// batch read judges the ARN its entry carried and takes no decision at all for an entry
/// that named the registry by id.
#[tokio::test]
async fn the_resource_judged_is_the_one_each_read_names() {
    let other_registry = "othrreg000001";
    // (case, the identifier the request names, the Resource the allow is scoped to,
    // allowed on the two registry-scoped reads, allowed on the batch read). IAM compares
    // resource ARNs literally, with `*` and `?` as the only wildcards; a bare id is served
    // by the batch read whatever the statement covers, because that read takes no decision
    // for it.
    let rows: Vec<(&str, String, Value, bool, bool)> = vec![
        (
            "the bare id under an allow of every resource",
            REGISTRY.to_string(),
            json!("*"),
            true,
            true,
        ),
        (
            "the ARN form under an allow of every resource",
            registry_arn(REGISTRY),
            json!("*"),
            true,
            true,
        ),
        (
            "the bare id under an allow of exactly that registry's ARN",
            REGISTRY.to_string(),
            json!(registry_arn(REGISTRY)),
            true,
            true,
        ),
        (
            "the ARN form under an allow of exactly that registry's ARN",
            registry_arn(REGISTRY),
            json!(registry_arn(REGISTRY)),
            true,
            true,
        ),
        (
            "the bare id under an allow of every registry of the account",
            REGISTRY.to_string(),
            json!(registry_arn("*")),
            true,
            true,
        ),
        (
            "the ARN form under an allow of every registry of the account",
            registry_arn(REGISTRY),
            json!(registry_arn("*")),
            true,
            true,
        ),
        (
            "the bare id under an allow of another registry only",
            REGISTRY.to_string(),
            json!(registry_arn(other_registry)),
            false,
            true,
        ),
        (
            "the ARN form under an allow of another registry only",
            registry_arn(REGISTRY),
            json!(registry_arn(other_registry)),
            false,
            false,
        ),
    ];

    for (case, identifier, resource, scoped_read_allowed, batch_read_allowed) in rows {
        for (operation, route, request, served_member) in reads_of(&identifier) {
            let case = format!("{operation} with {case}");
            let allowed = if operation == BATCH_READ {
                batch_read_allowed
            } else {
                scoped_read_allowed
            };
            let (router, engine) = router_with_identity();
            given_session(&engine);
            given_role_policy(
                &engine,
                Some(&json!({
                    "Statement": [{
                        "Effect": "Allow",
                        "Action": "agent-registry:*",
                        "Resource": resource,
                    }],
                })),
            );

            let (status, answer) = call_as(&router, SESSION_KEY_ID, &route, &request).await;
            if allowed {
                assert_eq!(
                    status, 200,
                    "case {case:?}: an allowed request must be served, got {status} {answer}"
                );
                assert!(
                    answer.get(served_member).is_some(),
                    "case {case:?}: a served call must report {served_member}, got {answer}"
                );
            } else {
                assert_refusal(
                    &case,
                    status,
                    &answer,
                    &refusal_message(
                        operation,
                        &identifier,
                        &session_arn(),
                        Refusal::DefaultDeny,
                    ),
                );
            }
        }
    }
}

/// RULE: the batch read takes its decision only for an entry that names its registry by
/// ARN, and words its refusal with its own fixed sentence. Recorded AWS behaviour: one
/// restricted caller, whose policies allow this action on no registry it names, is refused
/// when its entry names a registry by ARN and SERVED when it names that same registry by
/// id.
#[tokio::test]
async fn the_batch_read_decides_only_for_an_entry_naming_its_registry_by_arn() {
    let other_registry = "othrreg000001";
    // (case, the document in force on the restricted caller, the refusal the ARN form
    // meets). Every row leaves this action allowed on no registry the entries name.
    let rows: Vec<(&str, Option<Value>, Refusal)> = vec![
        ("no policy at all", None, Refusal::DefaultDeny),
        (
            "an allow of another registry only",
            Some(json!({
                "Statement": [{
                    "Effect": "Allow",
                    "Action": "agent-registry:*",
                    "Resource": registry_arn(other_registry),
                }],
            })),
            Refusal::DefaultDeny,
        ),
        (
            "an allow of every resource beside an explicit deny of this registry",
            Some(json!({
                "Statement": [
                    { "Effect": "Allow", "Action": "agent-registry:*", "Resource": "*" },
                    {
                        "Effect": "Deny",
                        "Action": "agent-registry:*",
                        "Resource": registry_arn(REGISTRY),
                    },
                ],
            })),
            Refusal::ExplicitDeny,
        ),
    ];

    for (row, document, refusal) in rows {
        // The ARN form: the entry carries the resource, so the decision is taken and
        // refuses.
        let (router, engine) = router_with_identity();
        given_session(&engine);
        given_role_policy(&engine, document.as_ref());
        let arn_form = registry_arn(REGISTRY);
        let (route, request, served_member) = read_of(BATCH_READ, &arn_form);
        let (status, answer) = call_as(&router, SESSION_KEY_ID, &route, &request).await;
        assert_refusal(
            &format!("the ARN form with {row}"),
            status,
            &answer,
            &refusal_message(BATCH_READ, &arn_form, &session_arn(), refusal),
        );

        // The id form of the SAME registry, under the SAME document: served.
        let (router, engine) = router_with_identity();
        given_session(&engine);
        given_role_policy(&engine, document.as_ref());
        let (route, request, _) = read_of(BATCH_READ, REGISTRY);
        let (status, answer) = call_as(&router, SESSION_KEY_ID, &route, &request).await;
        let case = format!("the id form with {row}");
        assert_eq!(
            status, 200,
            "case {case:?}: the id form must be served, got {status} {answer}"
        );
        assert!(
            answer.get(served_member).is_some(),
            "case {case:?}: a served call must report {served_member}, got {answer}"
        );
    }
}

/// RULE: an IAM user's long-term access key speaks for THAT user, so the decision is taken
/// on the user's own policies and the refusal names the user's ARN — the same rule the
/// session rows exercise, under the other principal form.
#[tokio::test]
async fn an_iam_user_key_is_judged_on_the_users_own_policies() {
    for (operation, ..) in reads_of(REGISTRY) {
        let identifier = judged_identifier(operation, REGISTRY);
        let (route, request, served_member) = read_of(operation, &identifier);
        // (case, the document in force, the refusal it meets)
        let rows: Vec<(&str, Option<Value>, Option<Refusal>)> = vec![
            ("no policy at all", None, Some(Refusal::DefaultDeny)),
            (
                "an explicit deny of this service",
                Some(json!({
                    "Statement": [
                        { "Effect": "Deny", "Action": "agent-registry:*", "Resource": "*" },
                    ],
                })),
                Some(Refusal::ExplicitDeny),
            ),
            (
                "an allow of every action of this service",
                Some(allow(json!("agent-registry:*"))),
                None,
            ),
        ];

        for (row, document, refused) in rows {
            let case = format!("{operation} as an IAM user with {row}");
            let (router, engine) = router_with_identity();
            given_user_key(&engine);
            given_user_policy(&engine, document.as_ref());

            let (status, answer) = call_as(&router, USER_KEY_ID, &route, &request).await;
            match refused {
                Some(refusal) => assert_refusal(
                    &case,
                    status,
                    &answer,
                    &refusal_message(operation, &identifier, &user_arn(), refusal),
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
    for (operation, route, request, served_member) in reads_of(REGISTRY) {
        let (router, engine) = router_with_identity();
        given_session(&engine);
        // The role carries nothing at all, which is what refuses the SESSION above.
        given_role_policy(&engine, None);

        let (status, answer) = call_as(&router, ACCOUNT_KEY_ID, &route, &request).await;
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

/// RULE: validation comes first — a request outside the model's own bounds is refused
/// `400 ValidationException` whatever the caller may do, so an unauthorized caller is not
/// handed a 403 for a request that was never well formed.
#[tokio::test]
async fn a_malformed_request_is_refused_before_the_authorization_decision() {
    // (case, route, body) — each outside a bound the model declares: `registryId`'s
    // `@pattern`, `registryIds`' `@length{min:1,max:1}` and `entries`' `@required`.
    let cases: [(&str, String, Value); 3] = [
        (
            "a registryId outside RegistryIdentifier's pattern",
            format!(
                "/registries/{}/discoverable-records-list",
                encode_label("bad id!")
            ),
            json!({}),
        ),
        (
            "registryIds over its maximum of one",
            "/discoverable-records-search".to_string(),
            json!({ "searchQuery": "", "maxResults": 10,
                    "registryIds": [REGISTRY, ABSENT_REGISTRY] }),
        ),
        (
            "entries absent altogether",
            "/discoverable-records-batch".to_string(),
            json!({}),
        ),
    ];

    for (case, route, request) in cases {
        let (router, engine) = router_with_identity();
        given_session(&engine);
        // The caller may do nothing at all, so a decision taken first would answer 403.
        given_role_policy(&engine, None);

        let (status, answer) = call_as(&router, SESSION_KEY_ID, &route, &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {answer}"
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {answer}"
        );
    }
}
