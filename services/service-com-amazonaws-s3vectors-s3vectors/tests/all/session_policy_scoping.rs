// SPDX-License-Identifier: CC0-1.0

//! What a request signed with a TEMPORARY credential may do, driven over the real
//! `restJson1` wire through the router production uses, with the session minted by a real
//! `sts:AssumeRole` call over its own wire.
//!
//! THE RULE: a request signed with a credential whose mint passed session policies is
//! permitted only what BOTH the documents in force on the identity AND those session
//! policies allow — "the resulting session's permissions are the intersection of the role's
//! identity-based policy and the session policies" (`AssumeRoleRequest$Policy`) — and an
//! explicit `Deny` in either refuses whatever the other allows. The session policies grant
//! nothing of their own, so a call only they allow is refused as if they had allowed
//! nothing, and a credential no mint scoped is capped by nothing.
//!
//! That is what makes the permission a call needs a property of the CREDENTIAL rather than
//! of the role: every permission the call stands on has to clear both documents, including
//! the ones it stands on only because of the parameters it carries — `s3vectors:GetVectors`
//! for a listing or a query that asks for vector data, for metadata or for metadata
//! filtering, and `s3vectors:TagResource` for a create that carries tags.
//!
//! The rows span what the rule generalises over: which document withholds the permission
//! (the session policy, the identity's own, an explicit deny in either), which permission is
//! withheld (the operation's own action, and each action a request parameter adds), the
//! parameter form that adds it (`returnData`, `returnMetadata`, both, a metadata filter, a
//! create's `tags`), and the shape of the scope itself (an inline document, a managed
//! document the account does not hold, and no scope at all).

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam;
use smithy_server_runtime::MultiServiceRouter;

use crate::vector_lifecycle::{error_code, expected_bucket_arn, expected_index_arn, REGION};

/// The bucket, the index in it and the vector every row is driven against.
const BUCKET: &str = "session-scoped-bucket";
const INDEX: &str = "session-scoped-index";
const PROBE: &str = "probe";

/// The role the sessions are of, and the name of the inline policy in force on it.
const ROLE: &str = "session-scoped-role";
const ROLE_POLICY_NAME: &str = "inline";

/// The access key id of the account's own published credential: a key neither STS nor IAM
/// holds a record for, which is the position AWS account root credentials are in. The
/// fixture is built with it, and `sts:AssumeRole` is called with it.
const ACCOUNT_KEY_ID: &str = "AKIDTESTS3VECTORS";

/// `sts:AssumeRole`'s own awsQuery API version, the `Version` the mint carries.
const STS_API_VERSION: &str = "2011-06-15";

/// A managed session policy ARN in the AWS-owned namespace, which is where AWS's own
/// managed policies live: a document this account's IAM store does not hold, so a scope
/// naming it is a scope this emulator cannot read.
const UNHELD_MANAGED_POLICY_ARN: &str = "arn:aws:iam::aws:policy/EmulatorProbeVectorPolicy";

/// A router serving this service beside the IAM and STS services over ONE shared engine, so
/// the role, its policies and the session a mint records are the ones those services hold.
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

/// A SigV4 `Authorization` header naming `access_key_id` for `signing_name`, which is what
/// names the principal the request speaks for. The signature is not verified: this router
/// runs with request authentication off.
fn authorization(access_key_id: &str, signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/{signing_name}/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=0000"
    )
}

/// One `s3vectors` call, signed with `access_key_id`, reported as `(status, body)`.
async fn call_as(
    router: &MultiServiceRouter,
    access_key_id: &str,
    path: &str,
    body: &Value,
) -> (u16, Value) {
    let payload =
        bytes::Bytes::from(serde_json::to_vec(body).expect("the request body serializes"));
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://s3vectors.{REGION}.amazonaws.com{path}"))
        .header("authorization", authorization(access_key_id, "s3vectors"))
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

/// Percent-encode the characters that are significant in an
/// `application/x-www-form-urlencoded` body, leaving everything else as a client's own
/// encoder would (`:` and `/` in an ARN, `{` and `"` in a policy document).
fn form_value(value: &str) -> String {
    value
        .chars()
        .map(|c| match c {
            '&' => "%26".to_string(),
            '=' => "%3D".to_string(),
            '+' => "%2B".to_string(),
            '%' => "%25".to_string(),
            ' ' => "%20".to_string(),
            '\n' => "%0A".to_string(),
            '\r' => "%0D".to_string(),
            other => other.to_string(),
        })
        .collect()
}

/// One `sts` awsQuery call signed with the account's own credential, reported as
/// `(status, body)` — the body as the XML text the wire carried.
async fn sts_call(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(String, String)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={STS_API_VERSION}");
    for (name, value) in params {
        body.push('&');
        body.push_str(name);
        body.push('=');
        body.push_str(&form_value(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("https://sts.amazonaws.com/")
        .header("host", "sts.amazonaws.com")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization(ACCOUNT_KEY_ID, "sts"))
        .body(bytes::Bytes::from(body))
        .expect("a well-formed awsQuery request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// The text of the one `<name>…</name>` element in an awsQuery response. Every element
/// these responses carry is uniquely named, so this reads the member without a document
/// model.
fn element(body: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)? + start;
    Some(body[start..end].to_string())
}

/// The credential one session was minted under: the access key id a later request signs
/// with, and the ARN `sts:GetCallerIdentity` reports for it — which is the principal a
/// refusal blames.
#[derive(Debug, Clone)]
struct Session {
    access_key_id: String,
    arn: String,
}

/// The ARN the account holds the role under.
fn role_arn() -> String {
    format!("arn:aws:iam::{}:role/{ROLE}", ::engine::emulator_account_id())
}

/// A role trust policy admitting this account's own principals to `sts:AssumeRole`, written
/// as `iam:CreateRole` stores one.
fn trust_policy() -> String {
    let account_root = format!("arn:aws:iam::{}:root", ::engine::emulator_account_id());
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "AWS": account_root },
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string()
}

/// Record the role every session of this suite is a session of, as `iam:CreateRole` records
/// one.
fn given_role(engine: &Arc<engine::InMemoryEngine>) {
    let store = iam::service_state(&**engine).expect("the IAM store");
    store.lock().roles.insert(
        iam::name_key(ROLE),
        ::service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::Role {
            path: "/".to_string(),
            role_name: ROLE.to_string(),
            role_id: "AROAEXAMPLESESSIONSCOPE".to_string(),
            arn: role_arn(),
            create_date: ::aws_smithy_types::DateTime::from_secs(0),
            assume_role_policy_document: Some(trust_policy()),
            description: None,
            max_session_duration: None,
            permissions_boundary: None,
            tags: None,
            role_last_used: None,
            source_role_template: None,
        },
    );
}

/// Put `document` in force as the role's inline policy, replacing whatever it carried —
/// what `iam:PutRolePolicy` does.
fn given_role_policy(engine: &Arc<engine::InMemoryEngine>, document: &Value) {
    let store = iam::service_state(&**engine).expect("the IAM store");
    let mut store = store.lock();
    store.role_inline_policies.insert(
        iam::name_key(ROLE),
        BTreeMap::from([(
            iam::name_key(ROLE_POLICY_NAME),
            iam::InlinePolicy {
                policy_name: ROLE_POLICY_NAME.to_string(),
                policy_document: document.to_string(),
            },
        )]),
    );
}

/// A policy allowing `actions` on every resource.
fn allow(actions: Value) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": "Allow", "Action": actions, "Resource": "*" }],
    })
}

/// A policy allowing everything this service does and DENYING `denied`.
fn allow_all_but(denied: Value) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [
            { "Effect": "Allow", "Action": "s3vectors:*", "Resource": "*" },
            { "Effect": "Deny", "Action": denied, "Resource": "*" },
        ],
    })
}

/// Mint a session of the role through a real `sts:AssumeRole` call, carrying `inline` as its
/// session policy and `managed` as its managed session policy ARNs.
async fn assume_role(
    router: &MultiServiceRouter,
    session_name: &str,
    inline: Option<&Value>,
    managed: &[&str],
) -> Session {
    let mut params = vec![
        ("RoleArn".to_string(), role_arn()),
        ("RoleSessionName".to_string(), session_name.to_string()),
    ];
    if let Some(inline) = inline {
        params.push(("Policy".to_string(), inline.to_string()));
    }
    for (position, arn) in managed.iter().enumerate() {
        params.push((
            format!("PolicyArns.member.{}.arn", position + 1),
            (*arn).to_string(),
        ));
    }
    let (status, body) = sts_call(router, "AssumeRole", &params).await;
    assert_eq!(
        status, 200,
        "case {session_name:?}: the mint must succeed, got {status} {body}"
    );
    Session {
        access_key_id: element(&body, "AccessKeyId")
            .unwrap_or_else(|| panic!("case {session_name:?}: no <AccessKeyId> in {body}")),
        arn: element(&body, "Arn")
            .unwrap_or_else(|| panic!("case {session_name:?}: no <Arn> in {body}")),
    }
}

/// A fresh router and engine holding the role, one bucket, one index in it and one vector
/// WITH metadata in the index — the fixture built with the account's own credential, which
/// no document bounds.
async fn fixture() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let (router, engine) = router_with_identity();
    given_role(&engine);
    for (path, body) in [
        ("/CreateVectorBucket", json!({ "vectorBucketName": BUCKET })),
        (
            "/CreateIndex",
            json!({
                "vectorBucketName": BUCKET,
                "indexName": INDEX,
                "dataType": "float32",
                "dimension": 3,
                "distanceMetric": "cosine",
            }),
        ),
        (
            "/PutVectors",
            json!({
                "vectorBucketName": BUCKET,
                "indexName": INDEX,
                "vectors": [{
                    "key": PROBE,
                    "data": { "float32": [1.0, 0.0, 0.0] },
                    "metadata": { "tenant": "blue" },
                }],
            }),
        ),
    ] {
        let (status, answered) = call_as(&router, ACCOUNT_KEY_ID, path, &body).await;
        assert_eq!(
            status, 200,
            "the fixture's {path} must succeed, got {status} {answered}"
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

/// IAM's refusal sentence: who was refused, which action, on which resource, and which of
/// IAM's rules refused it.
fn not_authorized(principal: &str, action: &str, resource: &str, reason: &str) -> String {
    format!(
        "User: {principal} is not authorized to perform: {action} on resource: {resource} {reason}"
    )
}

/// One request form under test: the operation's path, the body it carries, and the resource
/// a refusal of it names.
struct Form {
    case: &'static str,
    path: &'static str,
    body: Value,
    resource: String,
    /// The action this form stands on BESIDE the operation's own, because of the parameters
    /// it carries — `None` for a form that stands on its own action alone.
    extra_action: Option<&'static str>,
}

/// The request forms of a vector READ, one per way a request asks for vector data or
/// metadata: the two `ListVectors` flags and both together, and `QueryVectors`' metadata
/// flag, its metadata filter and both together. Each adds `s3vectors:GetVectors` to the
/// permissions the call stands on; the plain form of each operation stands on its own action
/// alone.
fn read_forms() -> Vec<Form> {
    let index = json!({ "vectorBucketName": BUCKET, "indexName": INDEX });
    let query_base = json!({
        "vectorBucketName": BUCKET,
        "indexName": INDEX,
        "topK": 1,
        "queryVector": { "float32": [1.0, 0.0, 0.0] },
    });
    // The members each row adds to its operation's plain form.
    let rows: Vec<(&'static str, &'static str, Value, Option<&'static str>)> = vec![
        ("ListVectors asking for neither", "/ListVectors", json!({}), None),
        (
            "ListVectors asking for data",
            "/ListVectors",
            json!({ "returnData": true }),
            Some("GetVectors"),
        ),
        (
            "ListVectors asking for metadata",
            "/ListVectors",
            json!({ "returnMetadata": true }),
            Some("GetVectors"),
        ),
        (
            "ListVectors asking for both",
            "/ListVectors",
            json!({ "returnData": true, "returnMetadata": true }),
            Some("GetVectors"),
        ),
        ("QueryVectors asking for neither", "/QueryVectors", json!({}), None),
        (
            "QueryVectors asking for metadata",
            "/QueryVectors",
            json!({ "returnMetadata": true }),
            Some("GetVectors"),
        ),
        (
            "QueryVectors filtering on metadata",
            "/QueryVectors",
            json!({ "filter": { "tenant": "blue" } }),
            Some("GetVectors"),
        ),
        (
            "QueryVectors filtering and asking for metadata",
            "/QueryVectors",
            json!({ "filter": { "tenant": "blue" }, "returnMetadata": true }),
            Some("GetVectors"),
        ),
    ];
    rows.into_iter()
        .map(|(case, path, added, extra_action)| {
            let mut body = match path {
                "/ListVectors" => index.clone(),
                _ => query_base.clone(),
            };
            for (name, value) in added.as_object().expect("the added members are an object") {
                body[name] = value.clone();
            }
            Form {
                case,
                path,
                body,
                resource: expected_index_arn(BUCKET, INDEX),
                extra_action,
            }
        })
        .collect()
}

/// The operation's own action, read off the path the form posts to.
fn own_action(path: &str) -> String {
    format!("s3vectors:{}", path.trim_start_matches('/'))
}

/// The intersection: with the role allowed everything, a session is served exactly the
/// calls its SESSION policy allows — every action the call stands on, including the one a
/// request parameter adds.
#[tokio::test]
async fn a_session_policy_caps_every_permission_the_call_stands_on() {
    for form in read_forms() {
        let own = own_action(form.path);
        // (the case, the session policy in force, the action it withholds)
        let scopes: Vec<(String, Value, Option<String>)> = vec![
            (
                "the own action alone".to_string(),
                allow(json!(own.clone())),
                form.extra_action
                    .map(|extra| format!("s3vectors:{extra}")),
            ),
            (
                "the own action and GetVectors".to_string(),
                allow(json!([own.clone(), "s3vectors:GetVectors"])),
                None,
            ),
            (
                "the whole service".to_string(),
                allow(json!("s3vectors:*")),
                None,
            ),
            (
                "another action of this service".to_string(),
                allow(json!("s3vectors:GetIndex")),
                Some(own.clone()),
            ),
        ];
        for (scope_case, scope, withheld) in scopes {
            let case = format!("{} under a scope of {scope_case}", form.case);
            let (router, engine) = fixture().await;
            given_role_policy(&engine, &allow(json!("s3vectors:*")));
            let session = assume_role(&router, "scoped", Some(&scope), &[]).await;

            let (status, body) =
                call_as(&router, &session.access_key_id, form.path, &form.body).await;
            match &withheld {
                Some(action) => {
                    assert_eq!(
                        (status, error_code(&body).as_str()),
                        (403, "AccessDeniedException"),
                        "case {case:?}: expected the modelled 403, got {status} {body}"
                    );
                    assert_eq!(
                        error_message(&body),
                        not_authorized(
                            &session.arn,
                            action,
                            &form.resource,
                            &format!("because no session policy allows the {action} action"),
                        ),
                        "case {case:?}: the refusal must name the session policy as the \
                         document that withheld {action}, got {body}"
                    );
                }
                None => assert_eq!(
                    status, 200,
                    "case {case:?}: a call the scope allows must be served, got {status} {body}"
                ),
            }
        }
    }
}

/// The other half of the intersection: a session policy grants nothing of its own, so a
/// call the ROLE's own policies do not allow is refused however wide the scope is — and the
/// refusal names the identity-based half, which needs a different fix.
#[tokio::test]
async fn a_session_policy_grants_nothing_the_identity_does_not_allow() {
    for form in read_forms() {
        let own = own_action(form.path);
        // The role allows the operation's own action and nothing else, so a form that
        // stands on GetVectors too is refused for THAT action.
        let case = format!("{} with the role allowed {own} alone", form.case);
        let (router, engine) = fixture().await;
        given_role_policy(&engine, &allow(json!(own.clone())));
        let session = assume_role(&router, "wide", Some(&allow(json!("s3vectors:*"))), &[]).await;

        let (status, body) = call_as(&router, &session.access_key_id, form.path, &form.body).await;
        match form.extra_action {
            Some(extra) => {
                let action = format!("s3vectors:{extra}");
                assert_eq!(
                    (status, error_code(&body).as_str()),
                    (403, "AccessDeniedException"),
                    "case {case:?}: expected the modelled 403, got {status} {body}"
                );
                assert_eq!(
                    error_message(&body),
                    not_authorized(
                        &session.arn,
                        &action,
                        &form.resource,
                        &format!("because no identity-based policy allows the {action} action"),
                    ),
                    "case {case:?}: the refusal must name the identity-based half, got {body}"
                );
            }
            None => assert_eq!(
                status, 200,
                "case {case:?}: a call both documents allow must be served, got {status} {body}"
            ),
        }
    }
}

/// An explicit `Deny` refuses whatever the other document allows, on either side of the
/// intersection — and each refusal names the document that denied it.
#[tokio::test]
async fn an_explicit_deny_on_either_side_refuses_the_call() {
    // (the case, the role's policy, the session policy, the clause the refusal closes with)
    let sides: Vec<(&str, Value, Value, &str)> = vec![
        (
            "a deny in the session policy",
            allow(json!("s3vectors:*")),
            allow_all_but(json!("s3vectors:GetVectors")),
            "with an explicit deny in a session policy",
        ),
        (
            "a deny in the role's own policy",
            allow_all_but(json!("s3vectors:GetVectors")),
            allow(json!("s3vectors:*")),
            "with an explicit deny in an identity-based policy",
        ),
    ];
    for (side, role_policy, scope, reason) in sides {
        for form in read_forms() {
            let case = format!("{} under {side}", form.case);
            let (router, engine) = fixture().await;
            given_role_policy(&engine, &role_policy);
            let session = assume_role(&router, "denied", Some(&scope), &[]).await;

            let (status, body) =
                call_as(&router, &session.access_key_id, form.path, &form.body).await;
            match form.extra_action {
                Some(_) => {
                    assert_eq!(
                        (status, error_code(&body).as_str()),
                        (403, "AccessDeniedException"),
                        "case {case:?}: expected the modelled 403, got {status} {body}"
                    );
                    assert_eq!(
                        error_message(&body),
                        not_authorized(
                            &session.arn,
                            "s3vectors:GetVectors",
                            &form.resource,
                            reason,
                        ),
                        "case {case:?}: the refusal must name the denying document, got {body}"
                    );
                }
                None => assert_eq!(
                    status, 200,
                    "case {case:?}: the keys-only call stands on no denied action and must be \
                     served, got {status} {body}"
                ),
            }
        }
    }
}

/// The rule is not about `GetVectors`: every permission a call stands on clears the scope,
/// including the `s3vectors:TagResource` a create that carries TAGS stands on — and a create
/// carrying none stands on its own action alone.
#[tokio::test]
async fn a_create_carrying_tags_clears_the_scope_for_the_tagging_action_too() {
    // (the case, the request, the action it stands on besides CreateIndex)
    let rows: Vec<(&str, Value, Option<&str>)> = vec![
        (
            "carrying no tags",
            json!({
                "vectorBucketName": BUCKET,
                "indexName": "session-scoped-untagged",
                "dataType": "float32",
                "dimension": 3,
                "distanceMetric": "cosine",
            }),
            None,
        ),
        (
            "carrying tags",
            json!({
                "vectorBucketName": BUCKET,
                "indexName": "session-scoped-tagged",
                "dataType": "float32",
                "dimension": 3,
                "distanceMetric": "cosine",
                "tags": { "env": "test" },
            }),
            Some("s3vectors:TagResource"),
        ),
    ];
    for (case, body, withheld) in rows {
        let (router, engine) = fixture().await;
        given_role_policy(&engine, &allow(json!("s3vectors:*")));
        // A scope allowing the create and nothing else.
        let session = assume_role(
            &router,
            "creating",
            Some(&allow(json!("s3vectors:CreateIndex"))),
            &[],
        )
        .await;

        let (status, answered) =
            call_as(&router, &session.access_key_id, "/CreateIndex", &body).await;
        match withheld {
            Some(action) => {
                assert_eq!(
                    (status, error_code(&answered).as_str()),
                    (403, "AccessDeniedException"),
                    "case {case:?}: expected the modelled 403, got {status} {answered}"
                );
                assert_eq!(
                    error_message(&answered),
                    not_authorized(
                        &session.arn,
                        action,
                        &expected_bucket_arn(BUCKET),
                        &format!("because no session policy allows the {action} action"),
                    ),
                    "case {case:?}: the refusal must name the tagging action, got {answered}"
                );
            }
            None => assert_eq!(
                status, 200,
                "case {case:?}: a create the scope allows must be served, got {status} {answered}"
            ),
        }
    }
}

/// A credential the mint scoped with nothing is capped by nothing, and one scoped only by a
/// MANAGED document this emulator does not hold is in that same position: refusing on
/// evidence it does not have would deny access AWS permits.
#[tokio::test]
async fn a_credential_no_readable_document_scopes_is_capped_by_nothing() {
    // (the case, the inline session policy, the managed session policy ARNs)
    let scopes: Vec<(&str, Option<Value>, Vec<&str>)> = vec![
        ("no session policy at all", None, Vec::new()),
        (
            "a managed session policy this account does not hold",
            None,
            vec![UNHELD_MANAGED_POLICY_ARN],
        ),
    ];
    for (case, inline, managed) in scopes {
        let (router, engine) = fixture().await;
        given_role_policy(&engine, &allow(json!("s3vectors:*")));
        let session = assume_role(&router, "unscoped", inline.as_ref(), &managed).await;

        for form in read_forms() {
            let case = format!("{} under {case}", form.case);
            let (status, body) =
                call_as(&router, &session.access_key_id, form.path, &form.body).await;
            assert_eq!(
                status, 200,
                "case {case:?}: a credential nothing readable scopes must be served, got \
                 {status} {body}"
            );
        }
    }
}

/// A scope caps the CREDENTIAL, not the role: two sessions of one role, minted with
/// different session policies, are served differently at the same instant — and the one the
/// scope allows reads back the data and the metadata the vector was stored with.
#[tokio::test]
async fn two_sessions_of_one_role_are_capped_by_their_own_scopes() {
    let (router, engine) = fixture().await;
    given_role_policy(&engine, &allow(json!("s3vectors:*")));
    let narrow = assume_role(
        &router,
        "narrow",
        Some(&allow(json!("s3vectors:ListVectors"))),
        &[],
    )
    .await;
    let wide = assume_role(
        &router,
        "wide",
        Some(&allow(json!(["s3vectors:ListVectors", "s3vectors:GetVectors"]))),
        &[],
    )
    .await;
    let asking = json!({
        "vectorBucketName": BUCKET,
        "indexName": INDEX,
        "returnData": true,
        "returnMetadata": true,
    });

    let (status, body) = call_as(&router, &narrow.access_key_id, "/ListVectors", &asking).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (403, "AccessDeniedException"),
        "case the narrow session: expected the modelled 403, got {status} {body}"
    );

    let (status, body) = call_as(&router, &wide.access_key_id, "/ListVectors", &asking).await;
    assert_eq!(
        status, 200,
        "case the wide session: expected the listing to be served, got {status} {body}"
    );
    let listed = body
        .get("vectors")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        listed.len(),
        1,
        "case the wide session: expected the one stored vector, got {body}"
    );
    assert_eq!(
        listed[0].get("metadata"),
        Some(&json!({ "tenant": "blue" })),
        "case the wide session: the metadata the vector was stored with must be reported, \
         got {body}"
    );
    assert_eq!(
        listed[0].pointer("/data/float32"),
        Some(&json!([1.0, 0.0, 0.0])),
        "case the wide session: the data the vector was stored with must be reported, got {body}"
    );
}
