// SPDX-License-Identifier: CC0-1.0

//! WHO may reach this service's operations, driven end to end over the real wire path (an
//! awsJson1_1 request through the multi-service router into the mounted service), so what
//! is asserted is what a client holding those credentials sees.
//!
//! THE RULE UNDER TEST: `AccessDeniedException` is a service-common error of Amazon
//! Textract, so EVERY operation evaluates the identity-based IAM policies in force on the
//! principal its credentials name against `textract:<OperationName>` BEFORE it reads or
//! writes anything — a request no statement allows (IAM's default deny), or one an
//! explicit `Deny` covers, is answered HTTP 400 `AccessDeniedException` and performs
//! nothing, while a request a matching `Allow` covers reaches the operation's own answer.
//!
//! The role, its policies and the session live in the IAM and STS stores of the SAME
//! shared engine — the stores those services' own handlers write — so the documents under
//! test are the ones `iam:PutRolePolicy` stores and the principal a refusal blames is the
//! one `sts:GetCallerIdentity` answers for the key the request signed with.
//!
//! The tables span what the rule generalises over: the operation (a synchronous read of a
//! document, both synchronous analyses, an asynchronous start, an asynchronous read, a
//! create, a resource-scoped read, a listing and a tagging write), the document in force
//! (none at all, another action of this service, another service's actions, an explicit
//! deny beside an allow, an allow of exactly the action, of the action in another case, of
//! the whole service and of everything), the resource an allow is scoped to (every
//! resource, this account's adapters, one adapter) and the principal (an assumed-role
//! session and the account's own published credential).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam;
use service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts;
use service_com_amazonaws_textract_textract::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::document_extraction::error_code;

/// The Region every call below is signed for.
const REGION: &str = "us-east-1";

/// The access key id of the account's own published credential: a key neither STS nor IAM
/// holds a record for, which is the position AWS account root credentials are in.
const ACCOUNT_KEY_ID: &str = "AKIDTEST";

/// The access key id of the assumed-role session every row signs with.
const SESSION_KEY_ID: &str = "ASIAEXAMPLESESSION01";

/// The role that session is assumed from, and the session's own name.
const ROLE: &str = "textract-reader";
const SESSION: &str = "textract-session";

/// The name of the inline policy the role carries.
const POLICY_NAME: &str = "inline";

/// How long the seeded session lasts: an hour, STS's own default.
const SESSION_SECONDS: i32 = 3_600;

/// A document every data-plane row submits: one line of text, as inline `Bytes`. Reading
/// it is not what these rows are about — the decision is taken before the document is
/// touched — but it must be a document the service CAN read, so an authorized row reaches a
/// 200 rather than a refusal about the document.
const DOCUMENT_TEXT: &str = "SIMPARITY LEDGER\nAccount Number: 48213\n";

/// A router serving textract beside the IAM and STS services, over ONE shared engine, so
/// the policies and the session the cases seed are the ones those services hold.
fn router_with_identity() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_textract_textract::EmulatedService::new(Arc::clone(&engine)),
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

/// Call one textract operation signed with `access_key_id` — what names the principal the
/// request speaks for.
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
        .header("x-amz-target", format!("Textract.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential={access_key_id}/20260101/{REGION}/textract/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0000"
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
            role_id: "AROAEXAMPLETEXTRACT01".to_string(),
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

/// Attach `document` to the role as its inline policy, replacing whatever it carried —
/// what `iam:PutRolePolicy` does. `None` takes every policy off it, as
/// `iam:DeleteRolePolicy` does.
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

/// The documents a row puts in force on the role, and which of IAM's refusals the request
/// then meets — `None` for a request a matching `Allow` permits.
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
                    { "Effect": "Deny", "Action": "textract:*", "Resource": "*" },
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
            Some(allow(json!(["s3:GetObject", action]))),
            None,
        ),
        (
            "an allow of every action of this service",
            Some(allow(json!("textract:*"))),
            None,
        ),
        ("an allow of every action", Some(allow(json!("*"))), None),
    ]
}

/// A `Document` of inline bytes this service can read.
fn readable_document() -> Value {
    json!({ "Bytes": aws_smithy_types::base64::encode(DOCUMENT_TEXT.as_bytes()) })
}

/// The operations this suite holds to the rule, with the request each is given: one
/// synchronous read of a document, both synchronous analyses, an asynchronous start, an
/// asynchronous read, a create, a listing, a resource-scoped read and a tagging write.
///
/// The request has to be one the operation would otherwise SERVE or answer on its own
/// terms, so an authorized row reaches the operation's own answer rather than another
/// refusal.
fn authorized_operations() -> Vec<(&'static str, Value)> {
    vec![
        (
            "DetectDocumentText",
            json!({ "Document": readable_document() }),
        ),
        (
            "AnalyzeDocument",
            json!({ "Document": readable_document(), "FeatureTypes": ["FORMS"] }),
        ),
        ("AnalyzeExpense", json!({ "Document": readable_document() })),
        (
            "StartDocumentTextDetection",
            json!({
                "DocumentLocation": { "S3Object": { "Bucket": "docs", "Name": "ledger.png" } },
            }),
        ),
        (
            "CreateAdapter",
            json!({ "AdapterName": "authorized-adapter", "FeatureTypes": ["QUERIES"] }),
        ),
        ("ListAdapters", json!({})),
    ]
}

/// THE RULE: every operation is judged against its OWN `textract:<OperationName>` action
/// before it performs anything; a refusal is HTTP 400 `AccessDeniedException` naming the
/// principal, the action and which of IAM's rules refused it, and an allowed call reaches
/// the operation's own answer.
#[tokio::test]
async fn every_operation_is_authorized_against_its_own_action() {
    for (operation, request) in authorized_operations() {
        let action = format!("textract:{operation}");
        // Another action of this service, which a row grants instead of the one under test.
        let other = authorized_operations()
            .into_iter()
            .map(|(name, _)| format!("textract:{name}"))
            .find(|candidate| *candidate != action)
            .expect("this service has more than one operation");

        for (row, document, refused) in documents_in_force(&action, &other) {
            let case = format!("{operation} with {row}");
            // A fresh store, role and session per row: no row can pass on another's write.
            let (router, engine) = router_with_identity();
            given_session(&engine);
            given_role_policy(&engine, document.as_ref());

            let (status, answer) = call_as(&router, SESSION_KEY_ID, operation, &request).await;
            match refused {
                Some(refusal) => {
                    assert_eq!(
                        status, 400,
                        "case {case:?}: expected HTTP 400, got {status} {answer}"
                    );
                    assert_eq!(
                        error_code(&answer),
                        "AccessDeniedException",
                        "case {case:?}: expected an AccessDeniedException, got {answer}"
                    );
                    assert_eq!(
                        answer.get("Message").and_then(Value::as_str),
                        Some(
                            format!(
                                "User: {} is not authorized to perform: {action} {}",
                                session_arn(),
                                refusal.reason(&action)
                            )
                            .as_str()
                        ),
                        "case {case:?}: expected IAM's wording, got {answer}"
                    );
                    // The refusal carries no part of the operation's own answer: an
                    // unauthorized caller is told nothing about the document or the store.
                    for member in ["Blocks", "ExpenseDocuments", "JobId", "AdapterId", "Adapters"] {
                        assert!(
                            answer.get(member).is_none(),
                            "case {case:?}: a refused call must report no {member}, got {answer}"
                        );
                    }
                }
                None => {
                    assert_eq!(
                        status, 200,
                        "case {case:?}: an allowed request must be served, got {status} {answer}"
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
    for (operation, request) in authorized_operations() {
        let (router, engine) = router_with_identity();
        given_session(&engine);
        // The role carries nothing at all, which is what refuses the SESSION above.
        given_role_policy(&engine, None);

        let (status, answer) = call_as(&router, ACCOUNT_KEY_ID, operation, &request).await;
        assert_eq!(
            status, 200,
            "case {operation:?}: the account's own credential must be served, got {status} \
             {answer}"
        );
    }
}

/// RULE: a resource-scoped action is judged against the ARN the request NAMES, so an allow
/// scoped to one adapter covers that adapter and no other, and an allow scoped to this
/// account's adapters covers both.
#[tokio::test]
async fn an_adapter_action_is_judged_against_the_adapter_the_request_names() {
    let mine = "0123456789ab";
    let other = "ba9876543210";
    let adapter_arn = |id: &str| state::adapter_arn(REGION, id);

    // (case, the Resource the allow is scoped to, whether the call for `mine` is allowed)
    let rows: Vec<(&str, Value, bool)> = vec![
        ("every resource", json!("*"), true),
        (
            "this account's adapters",
            json!(format!(
                "arn:aws:textract:{REGION}:{}:/adapters/*",
                ::engine::emulator_account_id()
            )),
            true,
        ),
        ("exactly this adapter", json!(adapter_arn(mine)), true),
        ("another adapter only", json!(adapter_arn(other)), false),
        (
            "an adapter collection in another Region",
            json!(format!(
                "arn:aws:textract:eu-west-1:{}:/adapters/*",
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
                "Statement": [
                    { "Effect": "Allow", "Action": "textract:GetAdapter", "Resource": resource },
                ],
            })),
        );

        let (status, answer) = call_as(
            &router,
            SESSION_KEY_ID,
            "GetAdapter",
            &json!({ "AdapterId": mine }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: no adapter was ever created, so every row answers a client error, \
             got {status} {answer}"
        );
        // Authorization answers BEFORE the store is read, so an allowed call falls through
        // to the operation's own verdict for an adapter nothing created, and a refused one
        // never gets that far — which is also what keeps a refusal from disclosing whether
        // the adapter exists.
        let expected = if allowed {
            "ResourceNotFoundException"
        } else {
            "AccessDeniedException"
        };
        assert_eq!(
            error_code(&answer),
            expected,
            "case {case:?}: expected {expected}, got {answer}"
        );
    }
}
