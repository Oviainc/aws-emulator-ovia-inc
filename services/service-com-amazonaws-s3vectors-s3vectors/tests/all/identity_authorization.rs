// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHO may reach this service's operations, driven over the real
//! `restJson1` wire through the router production uses, so what is asserted is what a
//! client holding those credentials sees.
//!
//! THE RULE: every operation of Amazon S3 Vectors is authorized against the identity-based
//! IAM policies in force on the principal its credentials name — and the permissions
//! boundary that caps them — on the action `s3vectors:<OperationName>` and the resource the
//! request ADDRESSES, BEFORE the operation reads or writes anything. A request no statement
//! allows (IAM's default deny) and one an explicit `Deny` covers are both answered
//! `AccessDeniedException` (HTTP 403) and perform nothing, while a request a matching
//! `Allow` covers reaches the operation's own answer. The refusal is IAM's own sentence,
//! built from the principal, the action, the resource and which of IAM's rules refused it.
//!
//! The user, the role, their policies and the session live in the IAM and STS stores of the
//! SAME shared engine, so the documents under test are the ones `iam:PutUserPolicy` and
//! `iam:PutRolePolicy` store, the access key is the one `iam:CreateAccessKey` records, and
//! the principal a refusal blames is the one `sts:GetCallerIdentity` answers for the key
//! the request signed with.
//!
//! The tables span what the rule generalises over: the operation (a vector listing, a
//! vector read, a query, a write, a delete, the index and bucket control plane, the policy
//! surface, the tagging surface and the account-level listing), the document in force (none
//! at all, another action of this service, another service's actions, this action scoped to
//! another bucket, an explicit deny beside an allow of everything, an allow of exactly the
//! action, of the action in a list, of the action spelled in another case, of the whole
//! service and of everything), and the principal (an IAM user signing with its own
//! long-term access key, an assumed-role session, and the account's own published
//! credential).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam;
use service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts;
use smithy_server_runtime::MultiServiceRouter;

use crate::vector_lifecycle::{encode_label, error_code, expected_bucket_arn, expected_index_arn, REGION};

/// The bucket, index and vector every row is driven against.
const BUCKET: &str = "identity-gated-bucket";
const INDEX: &str = "identity-gated-index";
const PROBE: &str = "probe";

/// The access key id of the account's own published credential, the key every other suite
/// in this crate signs with: a key neither STS nor IAM holds a record for, which is the
/// position AWS account root credentials are in.
const ACCOUNT_KEY_ID: &str = "AKIDTESTS3VECTORS";

/// The IAM user that signs with its OWN long-term access key, and that key's id.
const USER_NAME: &str = "s3vectors-reader";
const USER_KEY_ID: &str = "AKIAEXAMPLEUSERKEY01";

/// The assumed-role session the role rows sign with, the role it is a session of, and the
/// session's own name.
const SESSION_KEY_ID: &str = "ASIAEXAMPLESESSION01";
const ROLE: &str = "s3vectors-role";
const SESSION: &str = "vector-session";

/// The name of the inline policy a row puts in force.
const POLICY_NAME: &str = "inline";

/// A principal of ANOTHER account, for a bucket policy that is about somebody else: a
/// statement naming it says nothing about this account's callers.
const OTHER_ACCOUNT_ROOT: &str = "arn:aws:iam::999999999999:root";

/// How long the seeded session lasts: an hour, STS's own default.
const SESSION_SECONDS: i32 = 3_600;

/// A router serving this service beside the IAM and STS services, over ONE shared engine,
/// so the user, the session and the policies a row seeds are the ones those services hold.
fn router_with_identity() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_s3vectors_s3vectors::EmulatedService::new(
        Arc::clone(&engine),
    ));
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

/// A SigV4 `Authorization` header naming `access_key_id`, which is what names the principal
/// the request speaks for. The signature is not verified: this router runs with request
/// authentication off.
fn authorization(access_key_id: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/s3vectors/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=0000"
    )
}

/// Call one operation by the method and URI its `@http` binding declares, signed with
/// `access_key_id`, and report `(status, body)`.
async fn call_as(
    router: &MultiServiceRouter,
    access_key_id: &str,
    method: &str,
    path_and_query: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let payload = match body {
        Some(body) => {
            bytes::Bytes::from(serde_json::to_vec(body).expect("the request body serializes"))
        }
        None => bytes::Bytes::new(),
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!(
            "https://s3vectors.{REGION}.amazonaws.com{path_and_query}"
        ))
        .header("authorization", authorization(access_key_id))
        .header("content-type", "application/json")
        .body(payload)
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&payload).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// Make one call of `operation` against the fixture signed with `access_key_id`, in the form
/// its `@http` binding and required members take.
async fn attempt(
    router: &MultiServiceRouter,
    access_key_id: &str,
    operation: &str,
) -> (u16, Value) {
    let index_target = json!({ "vectorBucketName": BUCKET, "indexName": INDEX });
    let bucket_target = json!({ "vectorBucketName": BUCKET });
    let post = |body: Value| {
        let operation = operation.to_string();
        async move {
            call_as(
                router,
                access_key_id,
                "POST",
                &format!("/{operation}"),
                Some(&body),
            )
            .await
        }
    };
    match operation {
        "ListVectors" | "GetIndex" | "DeleteIndex" => post(index_target).await,
        "ListIndexes" | "GetVectorBucket" | "DeleteVectorBucket" | "GetVectorBucketPolicy"
        | "DeleteVectorBucketPolicy" => post(bucket_target).await,
        "GetVectors" | "DeleteVectors" => {
            post(json!({
                "vectorBucketName": BUCKET,
                "indexName": INDEX,
                "keys": [PROBE],
            }))
            .await
        }
        "QueryVectors" => {
            post(json!({
                "vectorBucketName": BUCKET,
                "indexName": INDEX,
                "topK": 1,
                "queryVector": { "float32": [1.0, 0.0, 0.0] },
            }))
            .await
        }
        "PutVectors" => {
            post(json!({
                "vectorBucketName": BUCKET,
                "indexName": INDEX,
                "vectors": [{ "key": "written", "data": { "float32": [0.0, 1.0, 0.0] } }],
            }))
            .await
        }
        "CreateIndex" => {
            post(json!({
                "vectorBucketName": BUCKET,
                "indexName": "identity-gated-second",
                "dataType": "float32",
                "dimension": 3,
                "distanceMetric": "cosine",
            }))
            .await
        }
        "CreateVectorBucket" => {
            post(json!({ "vectorBucketName": "identity-gated-created" })).await
        }
        "ListVectorBuckets" => post(json!({})).await,
        "PutVectorBucketDefaultIndexMode" => {
            post(json!({ "vectorBucketName": BUCKET, "defaultIndexMode": "CLASSIC" })).await
        }
        "PutVectorBucketPolicy" => {
            post(json!({
                "vectorBucketName": BUCKET,
                "policy": json!({
                    "Version": "2012-10-17",
                    "Statement": [{
                        "Effect": "Allow",
                        "Principal": "*",
                        "Action": "s3vectors:GetVectors",
                        "Resource": "*",
                    }],
                })
                .to_string(),
            }))
            .await
        }
        "TagResource" => {
            call_as(
                router,
                access_key_id,
                "POST",
                &format!("/tags/{}", encode_label(&expected_bucket_arn(BUCKET))),
                Some(&json!({ "tags": { "env": "test" } })),
            )
            .await
        }
        "UntagResource" => {
            call_as(
                router,
                access_key_id,
                "DELETE",
                &format!(
                    "/tags/{}?tagKeys=team",
                    encode_label(&expected_bucket_arn(BUCKET))
                ),
                None,
            )
            .await
        }
        "ListTagsForResource" => {
            call_as(
                router,
                access_key_id,
                "GET",
                &format!("/tags/{}", encode_label(&expected_bucket_arn(BUCKET))),
                None,
            )
            .await
        }
        other => panic!("no request form for {other}"),
    }
}

/// The resource each operation's refusal names: the index for an index-level action, the
/// bucket for a bucket-level one, the bucket this account would hold a created one under,
/// and nothing at all for the account-level listing.
fn acted_on(operation: &str) -> Option<String> {
    match operation {
        "ListVectors" | "GetVectors" | "QueryVectors" | "PutVectors" | "DeleteVectors"
        | "GetIndex" | "DeleteIndex" => Some(expected_index_arn(BUCKET, INDEX)),
        "CreateVectorBucket" => Some(expected_bucket_arn("identity-gated-created")),
        "ListVectorBuckets" => None,
        _ => Some(expected_bucket_arn(BUCKET)),
    }
}

/// Every operation the rule covers, in the order a row sweeps them.
const EVERY_OPERATION: [&str; 19] = [
    "ListVectors",
    "GetVectors",
    "QueryVectors",
    "PutVectors",
    "DeleteVectors",
    "GetIndex",
    "ListIndexes",
    "GetVectorBucket",
    "CreateIndex",
    "CreateVectorBucket",
    "ListVectorBuckets",
    "PutVectorBucketDefaultIndexMode",
    "GetVectorBucketPolicy",
    "PutVectorBucketPolicy",
    "DeleteVectorBucketPolicy",
    "TagResource",
    "UntagResource",
    "ListTagsForResource",
    "DeleteIndex",
];

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
            user_id: "AIDAEXAMPLES3VECTORSUSER".to_string(),
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
    let account = ::engine::emulator_account_id();
    let role_arn = role_arn();
    let iam_store = iam::service_state(&**engine).expect("the IAM store");
    iam_store.lock().roles.insert(
        iam::name_key(ROLE),
        ::service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::Role {
            path: "/".to_string(),
            role_name: ROLE.to_string(),
            role_id: "AROAEXAMPLES3VECTORSROLE".to_string(),
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
        .insert(vend.into_session(SESSION_KEY_ID, SystemTime::now()))
        .expect("the session is recorded");
}

/// Put `document` in force as the inline policy of the IAM user, replacing whatever it
/// carried — what `iam:PutUserPolicy` does. `None` takes every policy off it.
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

/// A policy allowing `actions` on `resource`.
fn allow_on(actions: Value, resource: &str) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": "Allow", "Action": actions, "Resource": resource }]
    })
}

/// A policy allowing `actions` on every resource.
fn allow(actions: Value) -> Value {
    allow_on(actions, "*")
}

/// A fresh router and engine holding one bucket, one index in it, one vector in the index
/// and an allow-only bucket policy, with the user and the session recorded.
async fn fixture() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let (router, engine) = router_with_identity();
    given_user_key(&engine);
    given_session(&engine);
    for (operation, body) in [
        (
            "CreateVectorBucket",
            json!({ "vectorBucketName": BUCKET }),
        ),
        (
            "CreateIndex",
            json!({
                "vectorBucketName": BUCKET,
                "indexName": INDEX,
                "dataType": "float32",
                "dimension": 3,
                "distanceMetric": "cosine",
            }),
        ),
        (
            "PutVectors",
            json!({
                "vectorBucketName": BUCKET,
                "indexName": INDEX,
                "vectors": [{ "key": PROBE, "data": { "float32": [1.0, 0.0, 0.0] } }],
            }),
        ),
        // A bucket policy that grants NOTHING to this caller — it names a principal of
        // another account — so the policy READS have a document to report while the
        // resource-based half of the decision states nothing about these rows: what each
        // one then turns on is the caller's own policies alone. What a bucket policy
        // GRANTS and DENIES has its own cases below.
        (
            "PutVectorBucketPolicy",
            json!({
                "vectorBucketName": BUCKET,
                "policy": json!({
                    "Version": "2012-10-17",
                    "Statement": [{
                        "Effect": "Allow",
                        "Principal": { "AWS": OTHER_ACCOUNT_ROOT },
                        "Action": "s3vectors:*",
                        "Resource": "*",
                    }],
                })
                .to_string(),
            }),
        ),
    ] {
        let (status, answered) = call_as(
            &router,
            ACCOUNT_KEY_ID,
            "POST",
            &format!("/{operation}"),
            Some(&body),
        )
        .await;
        assert_eq!(
            status, 200,
            "the fixture's {operation} must succeed, got {status} {answered}"
        );
    }
    (router, engine)
}

/// The error body's `message`, under either casing the envelope may carry it in.
fn error_message(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Which of IAM's two refusals a row expects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

/// The documents a row puts in force, and which of IAM's refusals the request then meets —
/// `None` for a request a matching `Allow` permits.
///
/// `action` is the `s3vectors:<OperationName>` under test; `others` the actions a row grants
/// instead of it.
fn identity_documents(action: &str, others: &[String]) -> Vec<(String, Option<Value>, Option<Refusal>)> {
    let other_list: Vec<Value> = others.iter().map(|other| json!(other)).collect();
    vec![
        (
            "no identity-based policy at all".to_string(),
            None,
            Some(Refusal::DefaultDeny),
        ),
        (
            "a document granting other actions of this service".to_string(),
            Some(allow(json!(other_list))),
            Some(Refusal::DefaultDeny),
        ),
        (
            "a document granting another service's actions".to_string(),
            Some(allow(json!("s3:GetObject"))),
            Some(Refusal::DefaultDeny),
        ),
        (
            "a document granting this action on ANOTHER bucket".to_string(),
            Some(allow_on(
                json!(action),
                &expected_bucket_arn("identity-gated-elsewhere"),
            )),
            Some(Refusal::DefaultDeny),
        ),
        (
            "an allow of everything with this action denied beside it".to_string(),
            Some(json!({
                "Version": "2012-10-17",
                "Statement": [
                    { "Effect": "Allow", "Action": "*", "Resource": "*" },
                    { "Effect": "Deny", "Action": action, "Resource": "*" },
                ],
            })),
            Some(Refusal::ExplicitDeny),
        ),
        (
            "a document granting exactly this action".to_string(),
            Some(allow(json!(action))),
            None,
        ),
        (
            "a document granting this action in a list".to_string(),
            Some(allow(json!([other_list.clone(), vec![json!(action)]]
                .concat()))),
            None,
        ),
        (
            "a document granting this action in another case".to_string(),
            Some(allow(json!(action.to_uppercase()))),
            None,
        ),
        (
            "a document granting every action of this service".to_string(),
            Some(allow(json!("s3vectors:*"))),
            None,
        ),
        (
            "a document granting everything".to_string(),
            Some(allow(json!("*"))),
            None,
        ),
    ]
}

/// Every operation, under every document: a principal is served exactly the calls its
/// identity-based policies allow, and refused every other with the modelled 403.
#[tokio::test]
async fn an_iam_user_is_served_exactly_the_actions_its_policies_allow() {
    for operation in EVERY_OPERATION {
        let action = format!("s3vectors:{operation}");
        // The actions a row grants INSTEAD of the one under test, so a grant of other
        // actions of this service is not accidentally a grant of this one.
        let others: Vec<String> = EVERY_OPERATION
            .iter()
            .filter(|other| **other != operation)
            .take(2)
            .map(|other| format!("s3vectors:{other}"))
            .collect();
        for (case, document, refused) in identity_documents(&action, &others) {
            // A fresh fixture per call, so a row's mutating operations cannot change what
            // the next one is answered.
            let (router, engine) = fixture().await;
            given_user_policy(&engine, document.as_ref());

            let (status, body) = attempt(&router, USER_KEY_ID, operation).await;
            match refused {
                Some(refusal) => {
                    assert_eq!(
                        (status, error_code(&body).as_str()),
                        (403, "AccessDeniedException"),
                        "case {case:?} on {operation}: expected the modelled 403, \
                         got {status} {body}"
                    );
                    let message = error_message(&body);
                    let expected = match acted_on(operation) {
                        Some(resource) => format!(
                            "User: {} is not authorized to perform: {action} on resource: \
                             {resource} {}",
                            user_arn(),
                            refusal.reason(&action)
                        ),
                        None => format!(
                            "User: {} is not authorized to perform: {action} {}",
                            user_arn(),
                            refusal.reason(&action)
                        ),
                    };
                    assert_eq!(
                        message, expected,
                        "case {case:?} on {operation}: expected IAM's sentence, got {message:?}"
                    );
                }
                None => assert_eq!(
                    status, 200,
                    "case {case:?} on {operation}: a granted call must be served, \
                     got {status} {body}"
                ),
            }
        }
    }
}

/// An assumed-role session is judged on the ROLE's policies, and the refusal names the
/// SESSION — the principal `sts:GetCallerIdentity` reports for the credential it signed
/// with.
#[tokio::test]
async fn an_assumed_role_session_is_judged_on_the_roles_policies() {
    let operation = "ListVectors";
    let action = format!("s3vectors:{operation}");
    for (case, document, refused) in identity_documents(&action, &["s3vectors:GetIndex".to_string()])
    {
        let (router, engine) = fixture().await;
        given_role_policy(&engine, document.as_ref());

        let (status, body) = attempt(&router, SESSION_KEY_ID, operation).await;
        match refused {
            Some(refusal) => {
                assert_eq!(
                    (status, error_code(&body).as_str()),
                    (403, "AccessDeniedException"),
                    "case {case:?}: expected the modelled 403, got {status} {body}"
                );
                let message = error_message(&body);
                assert_eq!(
                    message,
                    format!(
                        "User: {} is not authorized to perform: {action} on resource: {} {}",
                        session_arn(),
                        expected_index_arn(BUCKET, INDEX),
                        refusal.reason(&action)
                    ),
                    "case {case:?}: the refusal must name the session, got {message:?}"
                );
            }
            None => assert_eq!(
                status, 200,
                "case {case:?}: a granted call must be served, got {status} {body}"
            ),
        }
    }
}

/// The account's own published credential is not a principal the IAM store holds policies
/// for, so every operation stays served for it — which is what every other suite in this
/// crate signs with.
#[tokio::test]
async fn the_accounts_own_credential_is_served_every_operation() {
    for operation in EVERY_OPERATION {
        let (router, engine) = fixture().await;
        // A document on the USER in force at the same time, so what is asserted is that the
        // account credential is judged on its own position rather than on that user's.
        given_user_policy(&engine, None);
        let (status, body) = attempt(&router, ACCOUNT_KEY_ID, operation).await;
        assert_eq!(
            status, 200,
            "case {operation:?}: the account's own credential must be served, \
             got {status} {body}"
        );
    }
}

/// The decision is taken BEFORE the operation reads or writes anything: a refused write
/// leaves the index exactly as it was, and a refused read tells the caller nothing about
/// whether the resource it named exists.
#[tokio::test]
async fn a_refused_call_performs_nothing_and_resolves_nothing() {
    let (router, engine) = fixture().await;
    given_user_policy(&engine, None);

    // A write the policies do not allow stores no vector…
    let (status, body) = attempt(&router, USER_KEY_ID, "PutVectors").await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (403, "AccessDeniedException"),
        "the write must be refused, got {status} {body}"
    );
    let listed = call_as(
        &router,
        ACCOUNT_KEY_ID,
        "POST",
        "/ListVectors",
        Some(&json!({ "vectorBucketName": BUCKET, "indexName": INDEX })),
    )
    .await;
    assert_eq!(
        listed.1["vectors"],
        json!([{ "key": PROBE }]),
        "the refused write must have stored nothing, got {:?}",
        listed.1
    );

    // …and a read of a bucket that does NOT exist is the same 403, not a 404: the decision
    // is taken from the request alone, so a refusal tells an unauthorized caller nothing
    // about what the account holds.
    let (status, body) = call_as(
        &router,
        USER_KEY_ID,
        "POST",
        "/GetVectorBucket",
        Some(&json!({ "vectorBucketName": "identity-gated-absent" })),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (403, "AccessDeniedException"),
        "an unauthorized read must not resolve the resource, got {status} {body}"
    );
}

/// A read that asks for vector DATA or METADATA stands on `s3vectors:GetVectors` as well as
/// on its own action, so a grant of the operation's own action alone serves the request that
/// asks for neither and refuses the one that asks for either — which both ListVectors and
/// QueryVectors document, QueryVectors counting a metadata FILTER as asking for metadata.
#[tokio::test]
async fn asking_for_vector_data_or_metadata_stands_on_the_get_vectors_action_too() {
    let (router, engine) = fixture().await;

    // (operation, case, the members the request adds, refused)
    let cases = [
        ("ListVectors", "neither flag", json!({}), false),
        ("ListVectors", "returnData", json!({ "returnData": true }), true),
        (
            "ListVectors",
            "returnMetadata",
            json!({ "returnMetadata": true }),
            true,
        ),
        (
            "ListVectors",
            "both flags",
            json!({ "returnData": true, "returnMetadata": true }),
            true,
        ),
        (
            "ListVectors",
            "both flags set to false",
            json!({ "returnData": false, "returnMetadata": false }),
            false,
        ),
        ("QueryVectors", "keys alone", json!({}), false),
        (
            "QueryVectors",
            "distances, which this action's own permission covers",
            json!({ "returnDistance": true }),
            false,
        ),
        (
            "QueryVectors",
            "returnMetadata",
            json!({ "returnMetadata": true }),
            true,
        ),
        (
            "QueryVectors",
            "a metadata filter",
            json!({ "filter": { "group": "even" } }),
            true,
        ),
        (
            "QueryVectors",
            "a metadata filter and returnMetadata",
            json!({ "filter": { "group": "even" }, "returnMetadata": true }),
            true,
        ),
    ];
    for (operation, case, members, refused) in cases {
        // The caller is granted exactly the operation's OWN action, so what each row turns
        // on is the additional permission the request asks for.
        given_user_policy(&engine, Some(&allow(json!(format!("s3vectors:{operation}")))));
        let mut request = match operation {
            "QueryVectors" => json!({
                "vectorBucketName": BUCKET,
                "indexName": INDEX,
                "topK": 1,
                "queryVector": { "float32": [1.0, 0.0, 0.0] },
            }),
            _ => json!({ "vectorBucketName": BUCKET, "indexName": INDEX }),
        };
        for (member, value) in members.as_object().into_iter().flatten() {
            request[member] = value.clone();
        }
        let (status, body) = call_as(
            &router,
            USER_KEY_ID,
            "POST",
            &format!("/{operation}"),
            Some(&request),
        )
        .await;
        if refused {
            assert_eq!(
                (status, error_code(&body).as_str()),
                (403, "AccessDeniedException"),
                "case {operation} {case:?}: a request for vector data must be refused, \
                 got {status} {body}"
            );
            assert!(
                error_message(&body).contains("s3vectors:GetVectors"),
                "case {operation} {case:?}: the refusal must name the missing action, got {body}"
            );
        } else {
            assert_eq!(
                status, 200,
                "case {operation} {case:?}: a request for keys alone must stay served, \
                 got {status} {body}"
            );
        }
    }
}

/// A CREATE that carries tags writes tags too, so it stands on `s3vectors:TagResource` as
/// well as on its own action — and a create carrying none stands on its own action alone.
#[tokio::test]
async fn a_create_carrying_tags_stands_on_the_tag_resource_action_too() {
    // (operation, the request that carries tags, the request that carries none)
    let forms = [
        (
            "CreateVectorBucket",
            json!({
                "vectorBucketName": "identity-gated-tagged",
                "tags": { "env": "test" },
            }),
            json!({ "vectorBucketName": "identity-gated-untagged" }),
        ),
        (
            "CreateIndex",
            json!({
                "vectorBucketName": BUCKET,
                "indexName": "identity-gated-tagged",
                "dataType": "float32",
                "dimension": 3,
                "distanceMetric": "cosine",
                "tags": { "env": "test" },
            }),
            json!({
                "vectorBucketName": BUCKET,
                "indexName": "identity-gated-untagged",
                "dataType": "float32",
                "dimension": 3,
                "distanceMetric": "cosine",
            }),
        ),
    ];
    for (operation, tagged, untagged) in forms {
        let own_action = format!("s3vectors:{operation}");
        // (case, the request, the actions granted, refused)
        let cases = [
            (
                "no tags, the create action alone",
                &untagged,
                json!(own_action.clone()),
                false,
            ),
            (
                "tags, the create action alone",
                &tagged,
                json!(own_action.clone()),
                true,
            ),
            (
                "tags, the create action and TagResource",
                &tagged,
                json!([own_action.clone(), "s3vectors:TagResource".to_string()]),
                false,
            ),
            (
                "tags, every action of this service",
                &tagged,
                json!("s3vectors:*"),
                false,
            ),
        ];
        for (case, request, actions, refused) in cases {
            // A fresh fixture per row, so a create a previous row made cannot be the 409
            // the next one answers.
            let (router, engine) = fixture().await;
            given_user_policy(&engine, Some(&allow(actions)));
            let (status, body) = call_as(
                &router,
                USER_KEY_ID,
                "POST",
                &format!("/{operation}"),
                Some(request),
            )
            .await;
            if refused {
                assert_eq!(
                    (status, error_code(&body).as_str()),
                    (403, "AccessDeniedException"),
                    "case {operation} {case:?}: must be refused, got {status} {body}"
                );
                assert!(
                    error_message(&body).contains("s3vectors:TagResource"),
                    "case {operation} {case:?}: the refusal must name the missing action, \
                     got {body}"
                );
            } else {
                assert_eq!(
                    status, 200,
                    "case {operation} {case:?}: must be served, got {status} {body}"
                );
            }
        }
    }
}

/// A document put in force takes effect on the NEXT request, and taking it off lifts it
/// again: nothing about the decision is cached.
#[tokio::test]
async fn a_policy_takes_effect_on_the_next_request_and_lifts_when_removed() {
    let (router, engine) = fixture().await;
    let granted = allow(json!("s3vectors:ListVectors"));
    // (case, the document in force, the status it answers)
    let steps = [
        ("no policy yet", None, 403),
        ("the grant in force", Some(granted.clone()), 200),
        ("the grant taken off again", None, 403),
    ];
    for (case, document, expected) in steps {
        given_user_policy(&engine, document.as_ref());
        let (status, body) = attempt(&router, USER_KEY_ID, "ListVectors").await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected HTTP {expected}, got {status} {body}"
        );
    }
}

/// ONE decision, from BOTH documents: within this account a grant from either the caller's
/// own policies or the vector bucket's resource policy is enough, while an explicit `Deny`
/// on either side refuses whatever the other allows.
///
/// The rows span the combinations of what each side says (nothing / allow / deny), across
/// the vector data plane, the index and bucket control plane and the tagging surface, with
/// the bucket policy naming the caller by `*`, by its own ARN, by its account's two
/// spellings and by another account's — the account spellings decide a DENY and never a
/// grant, which is how IAM reads a statement about a whole account.
#[tokio::test]
async fn the_identity_and_resource_policies_are_one_decision() {
    // A bucket policy statement with `effect` on `action` for `principal`.
    let bucket_policy = |effect: &str, action: &str, principal: Value| {
        json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": effect,
                "Principal": principal,
                "Action": action,
                "Resource": "*",
            }],
        })
    };
    let account_root = format!("arn:aws:iam::{}:root", ::engine::emulator_account_id());

    // (case, operation, the caller's own document, the bucket's policy, served)
    let cases = vec![
        (
            "neither side says anything: IAM's default deny",
            "ListVectors",
            None,
            None,
            false,
        ),
        (
            "the bucket policy alone grants it, by *",
            "ListVectors",
            None,
            Some(bucket_policy("Allow", "s3vectors:ListVectors", json!("*"))),
            true,
        ),
        (
            "the bucket policy alone grants it, naming the caller's ARN",
            "GetVectors",
            None,
            Some(bucket_policy(
                "Allow",
                "s3vectors:GetVectors",
                json!({ "AWS": user_arn() }),
            )),
            true,
        ),
        (
            // An ALLOW naming an account delegates the decision to that account's own
            // identity-based policies rather than granting each of its principals, so a
            // caller holding no document of its own is still refused.
            "the bucket policy allows the caller's ACCOUNT ROOT, which delegates rather \
             than grants",
            "GetVectorBucket",
            None,
            Some(bucket_policy(
                "Allow",
                "s3vectors:*",
                json!({ "AWS": account_root.clone() }),
            )),
            false,
        ),
        (
            // The same spelling on a DENY is about every principal of the account.
            "the bucket policy denies the caller's ACCOUNT ROOT, which denies its \
             principals",
            "GetVectorBucket",
            Some(allow(json!("s3vectors:*"))),
            Some(bucket_policy(
                "Deny",
                "s3vectors:*",
                json!({ "AWS": account_root.clone() }),
            )),
            false,
        ),
        (
            "the bucket policy denies the caller's bare ACCOUNT ID, the other spelling of \
             it",
            "GetVectorBucket",
            Some(allow(json!("s3vectors:*"))),
            Some(bucket_policy(
                "Deny",
                "s3vectors:*",
                json!({ "AWS": ::engine::emulator_account_id() }),
            )),
            false,
        ),
        (
            "the bucket policy grants it to ANOTHER account, which is not this caller",
            "ListVectors",
            None,
            Some(bucket_policy(
                "Allow",
                "s3vectors:*",
                json!({ "AWS": OTHER_ACCOUNT_ROOT }),
            )),
            false,
        ),
        (
            "the bucket policy grants another action",
            "ListVectors",
            None,
            Some(bucket_policy("Allow", "s3vectors:GetVectors", json!("*"))),
            false,
        ),
        (
            "the caller's own policies alone grant it",
            "ListTagsForResource",
            Some(allow(json!("s3vectors:ListTagsForResource"))),
            None,
            true,
        ),
        (
            "both sides grant it",
            "QueryVectors",
            Some(allow(json!("s3vectors:QueryVectors"))),
            Some(bucket_policy("Allow", "s3vectors:*", json!("*"))),
            true,
        ),
        (
            "the bucket policy denies what the caller's own policies allow",
            "PutVectors",
            Some(allow(json!("s3vectors:*"))),
            Some(bucket_policy("Deny", "s3vectors:PutVectors", json!("*"))),
            false,
        ),
        (
            "the caller's own policies deny what the bucket policy allows",
            "GetIndex",
            Some(json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Deny",
                    "Action": "s3vectors:GetIndex",
                    "Resource": "*",
                }],
            })),
            Some(bucket_policy("Allow", "s3vectors:*", json!("*"))),
            false,
        ),
        (
            "the bucket policy grants the whole service to everyone",
            "ListIndexes",
            None,
            Some(bucket_policy("Allow", "s3vectors:*", json!("*"))),
            true,
        ),
    ];

    for (case, operation, own, policy, served) in cases {
        // A fresh fixture per row, so neither document outlives the row that set it.
        let (router, engine) = fixture().await;
        given_user_policy(&engine, own.as_ref());
        let (status, body) = call_as(
            &router,
            ACCOUNT_KEY_ID,
            "POST",
            "/PutVectorBucketPolicy",
            Some(&json!({
                "vectorBucketName": BUCKET,
                "policy": policy.clone().unwrap_or_else(|| json!({
                    "Version": "2012-10-17",
                    "Statement": [{
                        "Effect": "Allow",
                        "Principal": { "AWS": OTHER_ACCOUNT_ROOT },
                        "Action": "s3vectors:*",
                        "Resource": "*",
                    }],
                }))
                .to_string(),
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the row's bucket policy must be accepted, got {status} {body}"
        );

        let (status, body) = attempt(&router, USER_KEY_ID, operation).await;
        if served {
            assert_eq!(
                status, 200,
                "case {case:?}: {operation} must be served, got {status} {body}"
            );
        } else {
            assert_eq!(
                (status, error_code(&body).as_str()),
                (403, "AccessDeniedException"),
                "case {case:?}: {operation} must be refused, got {status} {body}"
            );
        }
    }
}
