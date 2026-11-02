// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHO may reach this service's operations, driven over the real wire
//! path (an awsJson1_1 request through the multi-service router into the mounted service),
//! so what is asserted is what a client holding those credentials sees.
//!
//! THE RULE: every operation of Secrets Manager is authorized against the identity-based
//! IAM policies in force on the principal its credentials name, on the action
//! `secretsmanager:<OperationName>` and the resource the request ADDRESSES, BEFORE the
//! operation reads or writes anything. A request no statement allows — or one an explicit
//! `Deny` covers — is answered the AWS common `AccessDeniedException` at this service's
//! client-error status (HTTP 400, awsJson1_1 discriminating by the error type in the body
//! rather than by the transport status) and performs nothing, while a request a matching
//! `Allow` covers reaches the operation's own answer. The refusal is IAM's own sentence,
//! built from the principal, the action, the resource and which of IAM's rules refused.
//!
//! The resource is the secret the request names, by the ARN the account holds it under —
//! whichever form of `SecretId` the request carried (the name, the full ARN, the partial
//! ARN) — and nothing at all for an action that addresses no secret, whose refusal
//! therefore states no resource clause.
//!
//! The user, the role, their policies and the session live in the IAM and STS stores of the
//! SAME shared engine, so the documents under test are the ones `iam:PutUserPolicy` and
//! `iam:PutRolePolicy` store, the access key is the one `iam:CreateAccessKey` records, and
//! the principal a refusal blames is the one `sts:GetCallerIdentity` answers for the key
//! the request signed with.
//!
//! The tables span what the rule generalises over: the operation (an account-wide list, a
//! generator, a batched read, a read of one secret, a per-secret list, a create, a tag
//! write, a value write and a delete), the document in force (none at all, other actions of
//! this service, another service's actions, this action scoped to another secret, an
//! explicit deny beside an allow of everything, an allow of exactly the action, of the
//! action in a list, of the action spelled in another case, of the whole service and of
//! everything), the principal (an IAM user signing with its own long-term access key, an
//! assumed-role session, and the account's own published credential) and the form of the
//! `SecretId` the request addresses the secret by.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam;
use service_com_amazonaws_secretsmanager_secretsmanager::EmulatedService;
use service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts;
use smithy_server_runtime::MultiServiceRouter;

use crate::secret_client_request_token::{fresh_token, with_staked_version};

/// The Region every case signs for, so an ARN is judged in the Region the caller actually
/// addressed rather than a default.
const REGION: &str = "us-west-2";

/// The error code an identity-based authorization refusal carries, as the AWS "Common
/// Errors" table names it.
const ACCESS_DENIED: &str = "AccessDeniedException";

/// The status this service answers that refusal with: awsJson1_1 discriminates a failure by
/// the error type in the body, so a denial travels with the ordinary client-error status.
const ACCESS_DENIED_STATUS: u16 = 400;

/// The access key id of the account's own published credential, as every other suite in this
/// crate signs with it: a key neither STS nor IAM holds a record for, which is the position
/// AWS account root credentials are in.
const ACCOUNT_KEY_ID: &str = "AKIDEXAMPLE";

/// The IAM user that signs with its OWN long-term access key — the principal the recorded
/// case mints with `iam:CreateUser` and `iam:CreateAccessKey` and attaches no policy to —
/// and that key's id.
const USER_NAME: &str = "secretsmanager-denied-user";
const USER_KEY_ID: &str = "AKIAEXAMPLEUSERKEY01";

/// The assumed-role session the other rows sign with, the role it is a session of, and the
/// session's own name.
const SESSION_KEY_ID: &str = "ASIAEXAMPLESESSION01";
const ROLE: &str = "secretsmanager-reader";
const SESSION: &str = "secrets-session";

/// The name of the inline policy a case puts in force.
const POLICY_NAME: &str = "inline";

/// How long the seeded session lasts: an hour, STS's own default.
const SESSION_SECONDS: i32 = 3_600;

/// The value the seeded secret holds, so an ALLOWED read is asserted on the material it
/// serves rather than on its status alone.
const SECRET_VALUE: &str = "the-stored-credential";

/// A router serving this service beside the IAM and STS services, over ONE shared engine, so
/// the user, the session and the policies the cases seed are the ones those services hold.
fn router_with_identity() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
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

/// Call one Secrets Manager operation signed with `access_key_id`, and return
/// `(status, body)`.
///
/// The same wire shape every other suite of this crate uses, with the access key id as a
/// parameter: it is what names the principal the request speaks for.
async fn call_as(
    router: &MultiServiceRouter,
    access_key_id: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("secretsmanager.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential={access_key_id}/20260101/{REGION}/secretsmanager/aws4_request, \
                 SignedHeaders=host;x-amz-date, Signature=deadbeef"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(&with_staked_version(operation, body))
                .expect("the request body is serializable"),
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

/// The `__type` discriminator of an awsJson1_1 error body, without its namespace.
fn error_code(parsed: &Value) -> String {
    let raw = parsed
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    raw.rsplit(['#', '/']).next().unwrap_or(raw).to_string()
}

/// The `message` of an error body, under either casing the envelope may carry it in.
fn error_message(parsed: &Value) -> String {
    parsed
        .get("message")
        .or_else(|| parsed.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// That user's ARN, as IAM holds it and `sts:GetCallerIdentity` reports it.
fn user_arn() -> String {
    format!(
        "arn:aws:iam::{}:user/{USER_NAME}",
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

/// The ARN the account holds the role under, which is the identity whose policies an
/// assumed-role session's request is judged on.
fn role_arn() -> String {
    format!(
        "arn:aws:iam::{}:role/{ROLE}",
        ::engine::emulator_account_id()
    )
}

/// Record the IAM user [`USER_KEY_ID`] belongs to, as `iam:CreateUser` and
/// `iam:CreateAccessKey` record one: the user, and the key record that maps the access key
/// id to it (the map IAM's own `GetUser` resolves a caller through). NO policy is attached.
fn given_user_key(engine: &Arc<engine::InMemoryEngine>) {
    let minted = ::aws_smithy_types::DateTime::from_secs(0);
    let store = iam::service_state(&**engine).expect("the IAM store");
    let mut store = store.lock();
    store.users.insert(
        iam::name_key(USER_NAME),
        ::service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::User {
            path: "/".to_string(),
            user_name: USER_NAME.to_string(),
            user_id: "AIDAEXAMPLESECRETSUSER".to_string(),
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
            role_id: "AROAEXAMPLESECRETSROLE".to_string(),
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
            external_id: None,
            role_arn: Some(role_arn),
            revoked: false,
        })
        .expect("the session is recorded");
}

/// Put `document` in force as the inline policy of the IAM user, replacing whatever it
/// carried — what `iam:PutUserPolicy` does. `None` takes every policy off it, as
/// `iam:DeleteUserPolicy` does.
fn given_user_policy(engine: &Arc<engine::InMemoryEngine>, document: Option<&Value>) {
    let store = iam::service_state(&**engine).expect("the IAM store");
    let mut store = store.lock();
    store.user_inline_policies.remove(&iam::name_key(USER_NAME));
    if let Some(document) = document {
        store.user_inline_policies.insert(
            iam::name_key(USER_NAME),
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

/// The same, for the role behind the session — what `iam:PutRolePolicy` does.
fn given_role_policy(engine: &Arc<engine::InMemoryEngine>, document: Option<&Value>) {
    let store = iam::service_state(&**engine).expect("the IAM store");
    let mut store = store.lock();
    store.role_inline_policies.remove(&iam::name_key(ROLE));
    if let Some(document) = document {
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

/// A policy allowing `actions` on every resource.
fn allow(actions: Value) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": "Allow", "Action": actions, "Resource": "*" }]
    })
}

/// A policy allowing `actions` on `resource` only.
fn allow_on(actions: Value, resource: &str) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": "Allow", "Action": actions, "Resource": resource }]
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

/// The documents a case puts in force, and which of IAM's refusals the request then meets —
/// `None` for a request a matching `Allow` permits.
///
/// `action` is the `secretsmanager:<OperationName>` under test; `others` the actions a row
/// grants instead of it.
fn identity_documents(
    action: &str,
    others: &[String],
    other_secret: &str,
) -> Vec<(&'static str, Option<Value>, Option<Refusal>)> {
    vec![
        ("no policy at all", None, Some(Refusal::DefaultDeny)),
        (
            "an allow of the other actions of this service, but not this one",
            Some(allow(json!(others))),
            Some(Refusal::DefaultDeny),
        ),
        (
            "an allow of every action of another service",
            Some(allow(json!("s3:*"))),
            Some(Refusal::DefaultDeny),
        ),
        (
            "an allow of this action scoped to another secret",
            Some(allow_on(json!(action), other_secret)),
            Some(Refusal::DefaultDeny),
        ),
        (
            "an explicit deny of this action beside an allow of everything",
            Some(json!({
                "Statement": [
                    { "Effect": "Allow", "Action": "secretsmanager:*", "Resource": "*" },
                    { "Effect": "Deny", "Action": action, "Resource": "*" },
                ]
            })),
            Some(Refusal::ExplicitDeny),
        ),
        (
            "an allow of exactly this action",
            Some(allow(json!(action))),
            None,
        ),
        (
            "an allow listing this action beside another service's",
            Some(allow(json!(["s3:GetObject", action]))),
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
            "an allow of every action of this service",
            Some(allow(json!("secretsmanager:*"))),
            None,
        ),
        ("an allow of every action", Some(allow(json!("*"))), None),
    ]
}

/// What the resource clause of an operation's refusal names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Addressed {
    /// The secret the request names, by the ARN the account holds it under.
    Secret,
    /// Nothing: the action reports the account, generates a password, or selects the
    /// secrets it answers for, rather than addressing one.
    NoResource,
    /// The secret a create allocates — the name it carries plus the six characters the
    /// create mints, which only the create itself knows — so the clause is asserted as the
    /// prefix the ARN of a secret of that name starts with.
    CreatedSecret,
}

/// The operations of this service a credential can reach, one per class the authorization
/// rule generalises over: the account-wide list the recorded case names, the password
/// generator, the batched read, a read of one secret's metadata, a read of its value, a
/// per-secret list, a read of its attached policy, a create, a tag write, a value write and
/// a delete.
///
/// `(operation, what its refusal names)`; the body each one sends is built by
/// [`request_body`].
const REACHABLE_OPERATIONS: &[(&str, Addressed)] = &[
    ("ListSecrets", Addressed::NoResource),
    ("GetRandomPassword", Addressed::NoResource),
    ("BatchGetSecretValue", Addressed::NoResource),
    ("DescribeSecret", Addressed::Secret),
    ("GetSecretValue", Addressed::Secret),
    ("ListSecretVersionIds", Addressed::Secret),
    ("GetResourcePolicy", Addressed::Secret),
    ("TagResource", Addressed::Secret),
    ("PutSecretValue", Addressed::Secret),
    ("DeleteSecret", Addressed::Secret),
    ("CreateSecret", Addressed::CreatedSecret),
];

/// The request `operation` sends, addressing the secret `secret` where it addresses one and
/// naming `created` where it creates one.
fn request_body(operation: &str, secret: &str, created: &str) -> Value {
    match operation {
        "ListSecrets" => json!({ "MaxResults": 1 }),
        "GetRandomPassword" => json!({ "PasswordLength": 16 }),
        "BatchGetSecretValue" => json!({ "SecretIdList": [secret] }),
        "DescribeSecret" | "GetSecretValue" | "ListSecretVersionIds" | "GetResourcePolicy"
        | "DeleteSecret" => json!({ "SecretId": secret }),
        "TagResource" => json!({
            "SecretId": secret,
            "Tags": [{ "Key": "owner", "Value": "platform" }],
        }),
        "PutSecretValue" => json!({
            "SecretId": secret,
            "SecretString": "a-rotated-credential",
            "ClientRequestToken": fresh_token(),
        }),
        "CreateSecret" => json!({
            "Name": created,
            "SecretString": SECRET_VALUE,
            "ClientRequestToken": fresh_token(),
        }),
        other => panic!("case {other:?}: this suite states no request for that operation"),
    }
}

/// Create one secret with the account's own credential, which this gate serves, and return
/// the ARN the account holds it under.
async fn given_secret(router: &MultiServiceRouter, name: &str) -> String {
    let (status, created) = call_as(
        router,
        ACCOUNT_KEY_ID,
        "CreateSecret",
        &json!({ "Name": name, "SecretString": SECRET_VALUE }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name:?}: the account's own credential must be served the create, got {status} \
         {created}"
    );
    created["ARN"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name:?}: the create reports no ARN: {created}"))
        .to_string()
}

/// What a refusal must say: the decision it reports, as the case states it.
#[derive(Debug, Clone)]
struct Denial<'a> {
    /// The principal that signed the refused request.
    principal: String,
    /// The `secretsmanager:<OperationName>` it asked for.
    action: String,
    /// What the resource clause names.
    addressed: Addressed,
    /// The ARN that clause carries, empty for an action that addresses no resource.
    arn: &'a str,
    /// Which of IAM's two rules refused it.
    refusal: Refusal,
}

/// Assert that `answer` is the refusal IAM words for the decision `denial` states.
fn assert_refused(case: &str, status: u16, answer: &Value, denial: &Denial<'_>) {
    let Denial {
        principal,
        action,
        addressed,
        arn,
        refusal,
    } = denial;
    assert_eq!(
        status, ACCESS_DENIED_STATUS,
        "case {case:?}: expected HTTP {ACCESS_DENIED_STATUS}, got {status} {answer}"
    );
    assert_eq!(
        error_code(answer),
        ACCESS_DENIED,
        "case {case:?}: expected an {ACCESS_DENIED}, got {answer}"
    );
    let reason = refusal.reason(action);
    let message = error_message(answer);
    match addressed {
        Addressed::NoResource => assert_eq!(
            message,
            format!("User: {principal} is not authorized to perform: {action} {reason}"),
            "case {case:?}: expected IAM's wording with no resource clause, got {message:?}"
        ),
        Addressed::Secret => assert_eq!(
            message,
            format!(
                "User: {principal} is not authorized to perform: {action} on resource: {arn} \
                 {reason}"
            ),
            "case {case:?}: expected IAM's wording naming the secret, got {message:?}"
        ),
        Addressed::CreatedSecret => {
            // The ARN a create allocates ends in the six characters it mints, which no
            // caller can predict, so the clause is asserted up to them.
            let opening = format!(
                "User: {principal} is not authorized to perform: {action} on resource: {arn}-"
            );
            assert!(
                message.starts_with(&opening) && message.ends_with(&reason),
                "case {case:?}: expected IAM's wording naming the secret being created \
                 ({opening}… {reason}), got {message:?}"
            );
        }
    }
}

/// THE RECORDED CASE, as a table: an IAM user signing with its own long-term access key and
/// holding no policy that allows `secretsmanager:ListSecrets` is refused the call — HTTP 400
/// `AccessDeniedException`, with the sentence IAM words from the principal and the action and
/// no resource clause, since the action addresses no secret — rather than served HTTP 200
/// with an empty `SecretList`, which an unauthorized caller could not tell from an account
/// holding no secret.
#[tokio::test]
async fn a_policy_less_iam_user_is_refused_the_account_wide_list() {
    let action = "secretsmanager:ListSecrets";
    let other_secret = format!(
        "arn:aws:secretsmanager:{REGION}:{}:secret:another-*",
        ::engine::emulator_account_id()
    );
    let others = ["secretsmanager:GetSecretValue".to_string()];
    for (row, document, refused) in identity_documents(action, &others, &other_secret) {
        let (router, engine) = router_with_identity();
        given_user_key(&engine);
        given_user_policy(&engine, document.as_ref());
        // A secret the account holds, so a refusal cannot be mistaken for an empty account
        // and an allowed list has something to report.
        given_secret(&router, "listed-secret").await;

        let case = format!("ListSecrets as a user with {row}");
        let (status, answer) =
            call_as(&router, USER_KEY_ID, "ListSecrets", &json!({ "MaxResults": 1 })).await;
        match refused {
            Some(refusal) => {
                assert_refused(
                    &case,
                    status,
                    &answer,
                    &Denial {
                        principal: user_arn(),
                        action: action.to_string(),
                        addressed: Addressed::NoResource,
                        arn: "",
                        refusal,
                    },
                );
                assert!(
                    answer["SecretList"].is_null(),
                    "case {case:?}: a refused list must report no secret, got {answer}"
                );
            }
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: an allowed list must be served, got {status} {answer}"
                );
                assert_eq!(
                    answer["SecretList"]
                        .as_array()
                        .map(|list| list.len())
                        .unwrap_or_default(),
                    1,
                    "case {case:?}: the allowed list must report the stored secret, got {answer}"
                );
            }
        }
    }
}

/// THE RULE across the operation classes: each is judged against its OWN action and the
/// resource it addresses, a refusal is the `AccessDeniedException` IAM words, and an allowed
/// request reaches the operation's own answer.
#[tokio::test]
async fn every_operation_is_authorized_against_its_own_action_and_resource() {
    let account = ::engine::emulator_account_id();
    // The resource a row scopes its allow to instead of the one the request addresses:
    // another secret of this account, which no case names.
    let other_secret = format!("arn:aws:secretsmanager:{REGION}:{account}:secret:elsewhere-*");
    for (operation, addressed) in REACHABLE_OPERATIONS {
        let action = format!("secretsmanager:{operation}");
        let others: Vec<String> = REACHABLE_OPERATIONS
            .iter()
            .filter(|(other, _)| other != operation)
            .map(|(other, _)| format!("secretsmanager:{other}"))
            .collect();

        for (index, (row, document, refused)) in
            identity_documents(&action, &others, &other_secret)
                .into_iter()
                .enumerate()
        {
            // A fresh store, session and secret per row: no row can pass on another's
            // write, and an allowed mutation cannot change what a later row addresses.
            let (router, engine) = router_with_identity();
            given_session(&engine);
            let secret_name = format!("addressed-{index}");
            let arn = given_secret(&router, &secret_name).await;
            given_role_policy(&engine, document.as_ref());

            let created = format!("created-{index}");
            // The resource a refusal names: the stored secret's ARN, the prefix of the ARN
            // a create allocates, or nothing at all.
            let named = match addressed {
                Addressed::Secret => arn.clone(),
                Addressed::CreatedSecret => {
                    format!("arn:aws:secretsmanager:{REGION}:{account}:secret:{created}")
                }
                Addressed::NoResource => String::new(),
            };

            let case = format!("{operation} as a session with {row}");
            let body = request_body(operation, &secret_name, &created);
            let (status, answer) = call_as(&router, SESSION_KEY_ID, operation, &body).await;
            match refused {
                Some(refusal) => assert_refused(
                    &case,
                    status,
                    &answer,
                    &Denial {
                        principal: session_arn(),
                        action: action.clone(),
                        addressed: *addressed,
                        arn: &named,
                        refusal,
                    },
                ),
                None => assert_eq!(
                    status, 200,
                    "case {case:?}: an allowed request must be served, got {status} {answer}"
                ),
            }
        }
    }
}

/// The account's own published credential holds no IAM policies, so it is served every
/// operation that a policy-less IAM user and a policy-less session are both refused: the
/// decision is about the principal, not about the operation.
#[tokio::test]
async fn the_accounts_own_credential_is_served_what_a_policy_less_principal_is_refused() {
    for (operation, _) in REACHABLE_OPERATIONS {
        let (router, engine) = router_with_identity();
        given_user_key(&engine);
        given_user_policy(&engine, None);
        given_session(&engine);
        given_role_policy(&engine, None);
        let secret_name = "shared-secret";
        given_secret(&router, secret_name).await;

        for (principal, key_id) in [(user_arn(), USER_KEY_ID), (session_arn(), SESSION_KEY_ID)] {
            let case = format!("{operation} as {principal}");
            let body = request_body(operation, secret_name, &format!("created-by-{key_id}"));
            let (status, answer) = call_as(&router, key_id, operation, &body).await;
            assert_eq!(
                status, ACCESS_DENIED_STATUS,
                "case {case:?}: a principal no policy allows must be refused, got {status} \
                 {answer}"
            );
            assert_eq!(
                error_code(&answer),
                ACCESS_DENIED,
                "case {case:?}: expected an {ACCESS_DENIED}, got {answer}"
            );
            assert!(
                error_message(&answer).starts_with(&format!("User: {principal} is not authorized")),
                "case {case:?}: the refusal must name the principal that signed, got {answer}"
            );
        }

        let case = format!("{operation} as the account's own credential");
        let body = request_body(operation, secret_name, "created-by-the-account");
        let (status, answer) = call_as(&router, ACCOUNT_KEY_ID, operation, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the account's own credential must be served, got {status} {answer}"
        );
    }
}

/// A refused call performs NOTHING: the secret it addressed is left exactly as it was — its
/// value, its tags and its deletion date — whatever the operation would have done, and
/// however many times it is retried.
#[tokio::test]
async fn a_refused_call_leaves_the_secret_exactly_as_it_was() {
    // The mutations of this service a refusal must leave no trace of.
    let mutations: &[&str] = &["PutSecretValue", "TagResource", "DeleteSecret"];
    for operation in mutations {
        let (router, engine) = router_with_identity();
        given_session(&engine);
        let secret_name = format!("untouched-by-{operation}");
        given_secret(&router, &secret_name).await;
        given_role_policy(&engine, None);

        for attempt in 1..=3 {
            let body = request_body(operation, &secret_name, "never-created");
            let (status, answer) = call_as(&router, SESSION_KEY_ID, operation, &body).await;
            assert_eq!(
                status, ACCESS_DENIED_STATUS,
                "case {operation} attempt {attempt}: expected a refusal, got {status} {answer}"
            );
        }

        // What the store holds now, read with the credential this gate serves.
        let (status, described) = call_as(
            &router,
            ACCOUNT_KEY_ID,
            "DescribeSecret",
            &json!({ "SecretId": secret_name }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {operation}: the account's own credential must be served the read, got \
             {status} {described}"
        );
        assert!(
            described["DeletedDate"].is_null(),
            "case {operation}: a refused call must schedule no deletion, got {described}"
        );
        assert!(
            described["Tags"]
                .as_array()
                .map(|tags| tags.is_empty())
                .unwrap_or(true),
            "case {operation}: a refused call must attach no tag, got {described}"
        );
        let (status, read) = call_as(
            &router,
            ACCOUNT_KEY_ID,
            "GetSecretValue",
            &json!({ "SecretId": secret_name }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {operation}: the stored value must still be served, got {status} {read}"
        );
        assert_eq!(
            read["SecretString"], json!(SECRET_VALUE),
            "case {operation}: a refused call must store no value, got {read}"
        );
    }
}

/// A grant scoped to ONE secret covers that secret under every form of `SecretId` a request
/// may address it by — the name, the full ARN and the partial ARN — because the decision is
/// made against the ARN the account holds the secret under; and it covers no other secret.
#[tokio::test]
async fn a_grant_scoped_to_one_secret_covers_it_under_every_id_form() {
    let (router, engine) = router_with_identity();
    given_session(&engine);
    let granted_arn = given_secret(&router, "granted-secret").await;
    let other_arn = given_secret(&router, "ungranted-secret").await;
    // The partial ARN of the granted secret: the full one without the hyphen and the six
    // characters a secret's ARN ends in, which a caller that never saw the full ARN can
    // still build and address the secret by.
    let partial_arn = granted_arn
        .get(..granted_arn.len() - 7)
        .expect("a secret ARN ends in a hyphen and six characters")
        .to_string();

    // The statement forms a policy scopes a grant to one secret with: its full ARN, the
    // wildcard over the six characters it ends in, and the single-character wildcard per
    // character (`?`), all three of which IAM matches against that ARN.
    let scopes: &[(&str, String)] = &[
        ("the secret's full ARN", granted_arn.clone()),
        (
            "a wildcard over the characters the ARN ends in",
            format!("{partial_arn}-*"),
        ),
        (
            "a single-character wildcard per character",
            format!("{partial_arn}-??????"),
        ),
    ];
    // Every form of `SecretId` the granted secret can be addressed by.
    let id_forms: &[(&str, String)] = &[
        ("the secret's name", "granted-secret".to_string()),
        ("the secret's full ARN", granted_arn.clone()),
        ("the secret's partial ARN", partial_arn.clone()),
    ];

    for (scope, resource) in scopes {
        given_role_policy(
            &engine,
            Some(&allow_on(json!("secretsmanager:GetSecretValue"), resource)),
        );
        for (form, secret_id) in id_forms {
            let case = format!("{scope} addressed by {form}");
            let (status, answer) = call_as(
                &router,
                SESSION_KEY_ID,
                "GetSecretValue",
                &json!({ "SecretId": secret_id }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the grant covers this secret, got {status} {answer}"
            );
            assert_eq!(
                answer["SecretString"],
                json!(SECRET_VALUE),
                "case {case:?}: the allowed read must serve the stored value, got {answer}"
            );
        }

        // The SAME grant covers no other secret, and the refusal names the ARN of the one
        // the request addressed rather than the one the grant covers.
        for (form, secret_id) in [
            ("its name", "ungranted-secret".to_string()),
            ("its ARN", other_arn.clone()),
        ] {
            let case = format!("{scope} against another secret addressed by {form}");
            let (status, answer) = call_as(
                &router,
                SESSION_KEY_ID,
                "GetSecretValue",
                &json!({ "SecretId": secret_id }),
            )
            .await;
            assert_refused(
                &case,
                status,
                &answer,
                &Denial {
                    principal: session_arn(),
                    action: "secretsmanager:GetSecretValue".to_string(),
                    addressed: Addressed::Secret,
                    arn: &other_arn,
                    refusal: Refusal::DefaultDeny,
                },
            );
            assert!(
                answer["SecretString"].is_null(),
                "case {case:?}: a refused read must serve no value, got {answer}"
            );
        }
    }
}
