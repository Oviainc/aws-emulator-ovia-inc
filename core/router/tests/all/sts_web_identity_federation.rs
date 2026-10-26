// SPDX-License-Identifier: CC0-1.0

//! Web identity federation end to end, across three services on one engine.
//!
//! RULE: `sts:AssumeRoleWithWebIdentity` exchanges a web identity token for a session of
//! the role the request names — unsigned, because the operation is modelled anonymous — and
//! what it exchanges is a token THIS emulator issued: the issuance registry written by the
//! Amazon Cognito Identity call that minted the token is the evidence the redeemer has, in
//! place of the issuer's published verification key, which this emulator fetches from
//! nowhere. The session is one of the role's own, so `sts:GetCallerIdentity` under the
//! credential reports the assumed-role identity, and the response reports the three claims
//! the token carries (`Provider`, `Audience`, `SubjectFromWebIdentityToken`).
//!
//! The trust policy decides, as it does for every assume: the role must admit the token's
//! ISSUER for `sts:AssumeRoleWithWebIdentity`, and a `Condition` on the audience the issuer
//! publishes (`cognito-identity.amazonaws.com:aud`) is evaluated against the token's own
//! audience — so a role conditioned on another pool's id admits nothing.
//!
//! Only a cross-service suite can pin this: the role is written by `iam:CreateRole`, the
//! token is minted by `cognito-identity:GetOpenIdToken`, the exchange is served by STS and
//! the session is resolved by `sts:GetCallerIdentity` — three services over ONE engine, as
//! `core/router/src/lib.rs` mounts them.
//!
//! Assembling the whole router costs stack (see `boot_serve.rs`), so it is built — and the
//! requests driven — on a sized thread.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The Region every signed call below is signed for.
const REGION: &str = "us-east-1";

/// The emulator's own address: an address, so the `Host` header names no service — which is
/// the shape an unsigned request from a client pointed at this emulator has, and the one
/// the anonymous legs below rely on being routed anyway.
const EMULATOR_HOST: &str = "127.0.0.1:9297";

/// The access key id every signed setup call carries.
const SETUP_KEY_ID: &str = "AKIDTEST";

/// STS's awsQuery API version.
const STS_API_VERSION: &str = "2011-06-15";

/// What one call answered.
struct Answer {
    status: u16,
    body: bytes::Bytes,
}

impl Answer {
    fn json(&self) -> Value {
        if self.body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&self.body).unwrap_or(Value::Null)
        }
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }
}

/// Dispatch one request, signed with `access_key_id` for `signing_name`, or UNSIGNED when
/// `signing_name` is `None` — which is how an SDK sends an operation whose model lists no
/// auth scheme, and how every anonymous leg below is sent.
fn dispatch(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    signing_name: Option<&str>,
    access_key_id: &str,
    headers: &[(&str, &str)],
    body: bytes::Bytes,
) -> Answer {
    let host = match signing_name {
        Some(name) => format!("{name}.{REGION}.amazonaws.com"),
        None => EMULATOR_HOST.to_string(),
    };
    let mut builder = http::Request::builder()
        .method("POST")
        .uri(format!("https://{host}/"))
        .header("host", host.clone());
    if let Some(signing_name) = signing_name {
        // The signature is never checked: this router runs with authentication off. What
        // the scope carries is the key id and the Region a handler reads.
        builder = builder.header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential={access_key_id}/20260101/{REGION}/{signing_name}/aws4_request, \
                 SignedHeaders=host, Signature=not-verified"
            ),
        );
    }
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder.body(body).expect("a well-formed request");
    let response = runtime.block_on(router.dispatch(request));
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        body,
    }
}

/// One awsJson1_1 call to `target`, signed for `signing_name` or unsigned.
fn rpc(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    signing_name: Option<&str>,
    target: &str,
    body: &Value,
) -> Answer {
    dispatch(
        router,
        runtime,
        signing_name,
        SETUP_KEY_ID,
        &[
            ("content-type", "application/x-amz-json-1.1"),
            ("x-amz-target", target),
        ],
        bytes::Bytes::from(body.to_string()),
    )
}

/// One awsQuery call — IAM's and STS's wire form — signed for `signing_name` with
/// `access_key_id`, or unsigned.
fn query(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    signing_name: Option<&str>,
    access_key_id: &str,
    body: String,
) -> Answer {
    dispatch(
        router,
        runtime,
        signing_name,
        access_key_id,
        &[("content-type", "application/x-www-form-urlencoded")],
        bytes::Bytes::from(body),
    )
}

/// The text of the first `<name>…</name>` element of an XML body, or `None` when it carries
/// no such element.
fn xml_element(body: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)? + start;
    Some(body[start..end].to_string())
}

/// The element, or a panic naming the case and the whole body.
fn member(case: &str, body: &str, name: &str) -> String {
    xml_element(body, name).unwrap_or_else(|| panic!("case {case:?}: no <{name}> in {body}"))
}

/// A role trust policy admitting the Cognito Identity principal for the identity pool
/// `audience`, written the way the Amazon Cognito Developer Guide writes one: the
/// `Federated` principal, the web-identity assume, and the `:aud` condition that restricts
/// the assume to users of that one pool.
///
/// Compact and free of the characters that are significant in an
/// `application/x-www-form-urlencoded` body (`&`, `=`, `+`, `%`, space), so it needs no
/// encoding.
fn trust_policy(audience: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
         \"Principal\":{{\"Federated\":\"cognito-identity.amazonaws.com\"}},\
         \"Action\":\"sts:AssumeRoleWithWebIdentity\",\"Condition\":{{\"StringEquals\":\
         {{\"cognito-identity.amazonaws.com:aud\":\"{audience}\"}}}}}}]}}"
    )
}

/// `iam:CreateRole` with `document` as its trust policy, answering the role's ARN.
fn create_role(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    role_name: &str,
    document: &str,
) -> String {
    let created = query(
        router,
        runtime,
        Some("iam"),
        SETUP_KEY_ID,
        format!(
            "Action=CreateRole&Version=2010-05-08&RoleName={role_name}\
             &AssumeRolePolicyDocument={document}"
        ),
    );
    assert_eq!(
        created.status,
        200,
        "setup: CreateRole({role_name:?}) must succeed, got {} with {:?}",
        created.status,
        created.text()
    );
    member("setup", &created.text(), "Arn")
}

/// `cognito-identity:CreateIdentityPool`, answering the pool's id.
fn create_pool(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    pool_name: &str,
) -> String {
    let created = rpc(
        router,
        runtime,
        Some("cognito-identity"),
        "AWSCognitoIdentityService.CreateIdentityPool",
        &json!({"IdentityPoolName": pool_name, "AllowUnauthenticatedIdentities": true}),
    );
    assert_eq!(
        created.status,
        200,
        "setup: CreateIdentityPool({pool_name:?}) must succeed, got {} with {:?}",
        created.status,
        created.text()
    );
    created.json()["IdentityPoolId"]
        .as_str()
        .expect("CreateIdentityPool reports an IdentityPoolId")
        .to_string()
}

/// An unsigned `GetId`, answering the identity it minted.
fn get_id(router: &MultiServiceRouter, runtime: &tokio::runtime::Runtime, pool_id: &str) -> String {
    let minted = rpc(
        router,
        runtime,
        None,
        "AWSCognitoIdentityService.GetId",
        &json!({"IdentityPoolId": pool_id}),
    );
    assert_eq!(
        minted.status,
        200,
        "an unsigned GetId must be served, got {} with {:?}",
        minted.status,
        minted.text()
    );
    minted.json()["IdentityId"]
        .as_str()
        .expect("GetId reports an IdentityId")
        .to_string()
}

/// An unsigned `GetOpenIdToken`, answering the web identity token it issued for the
/// identity.
fn open_id_token(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    identity_id: &str,
) -> String {
    let issued = rpc(
        router,
        runtime,
        None,
        "AWSCognitoIdentityService.GetOpenIdToken",
        &json!({"IdentityId": identity_id}),
    );
    assert_eq!(
        issued.status,
        200,
        "an unsigned GetOpenIdToken must be served, got {} with {:?}",
        issued.status,
        issued.text()
    );
    issued.json()["Token"]
        .as_str()
        .expect("GetOpenIdToken reports a Token")
        .to_string()
}

/// An UNSIGNED `sts:AssumeRoleWithWebIdentity`, the way a client holding no Amazon Web
/// Services credential makes it.
fn assume_with_web_identity(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    role_arn: &str,
    session_name: &str,
    token: &str,
) -> Answer {
    query(
        router,
        runtime,
        None,
        SETUP_KEY_ID,
        format!(
            "Action=AssumeRoleWithWebIdentity&Version={STS_API_VERSION}&RoleArn={role_arn}\
             &RoleSessionName={session_name}&WebIdentityToken={token}"
        ),
    )
}

#[test]
fn a_web_identity_token_this_emulator_issued_is_exchanged_for_a_role_session() {
    std::thread::Builder::new()
        .name("sts-web-identity-federation".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a current-thread runtime");

            // Two pools, and two roles: one conditioned on the FIRST pool's id, which is
            // the audience the token under test carries, and one conditioned on the
            // SECOND's, which it does not.
            let pool = create_pool(&router, &runtime, "web-identity-federation");
            let other_pool = create_pool(&router, &runtime, "web-identity-other");
            let role_arn = create_role(
                &router,
                &runtime,
                "web-identity-federated",
                &trust_policy(&pool),
            );
            let other_audience_role = create_role(
                &router,
                &runtime,
                "web-identity-other-audience",
                &trust_policy(&other_pool),
            );

            let identity_id = get_id(&router, &runtime, &pool);
            let token = open_id_token(&router, &runtime, &identity_id);

            // The served leg: the token this emulator issued is exchanged for a session of
            // the role whose trust policy admits that token's audience.
            let served = assume_with_web_identity(
                &router,
                &runtime,
                &role_arn,
                "federated-session",
                &token,
            );
            let body = served.text();
            assert_eq!(
                served.status, 200,
                "an unsigned assume with a token this emulator issued must be served, got \
                 {} with {body:?}",
                served.status
            );
            let case = "the issued token";
            let access_key_id = member(case, &body, "AccessKeyId");
            assert_eq!(
                member(case, &body, "SubjectFromWebIdentityToken"),
                identity_id,
                "case {case:?}: the subject the token names must be reported, got {body}"
            );
            assert_eq!(
                member(case, &body, "Audience"),
                pool,
                "case {case:?}: the audience the token names must be reported, got {body}"
            );
            assert_eq!(
                member(case, &body, "Provider"),
                "https://cognito-identity.amazonaws.com",
                "case {case:?}: the issuer the token names must be reported, got {body}"
            );
            let assumed_role_arn = member(case, &body, "Arn");
            assert!(
                assumed_role_arn.ends_with(":assumed-role/web-identity-federated/federated-session"),
                "case {case:?}: expected the assumed-role ARN of the role and session, got \
                 {assumed_role_arn:?}"
            );

            // The session is one of this service's own: resolved afterwards by
            // `GetCallerIdentity` signed with the credential the exchange minted.
            let resolved = query(
                &router,
                &runtime,
                Some("sts"),
                &access_key_id,
                format!("Action=GetCallerIdentity&Version={STS_API_VERSION}"),
            );
            let resolved_body = resolved.text();
            assert_eq!(
                resolved.status, 200,
                "the minted credential must resolve, got {} with {resolved_body:?}",
                resolved.status
            );
            assert_eq!(
                member(case, &resolved_body, "Arn"),
                assumed_role_arn,
                "case {case:?}: the session must report the assumed-role identity, got \
                 {resolved_body}"
            );

            // One row per way the exchange is refused: the trust policy's audience
            // condition, and a token this emulator never issued.
            let hand_made_token = format!(
                "{}.{}.{}",
                "eyJhbGciOiJSUzI1NiIsInR5cCI6IkpXVCJ9",
                "eyJpc3MiOiJodHRwczovL2NvZ25pdG8taWRlbnRpdHkuYW1hem9uYXdzLmNvbSIsImV4cCI6NDA3MDkwODgwMH0",
                "a-signature-nothing-here-can-verify"
            );
            let rows: Vec<(&str, &str, &str, u16, &str)> = vec![
                (
                    "a role conditioned on another pool's audience",
                    other_audience_role.as_str(),
                    token.as_str(),
                    403,
                    "AccessDenied",
                ),
                (
                    "a token this emulator never issued",
                    role_arn.as_str(),
                    hand_made_token.as_str(),
                    400,
                    "InvalidIdentityToken",
                ),
            ];
            for (case, role, presented, expected_status, expected_code) in rows {
                let refused =
                    assume_with_web_identity(&router, &runtime, role, "federated-session", presented);
                let refused_body = refused.text();
                assert_eq!(
                    refused.status, expected_status,
                    "case {case:?}: expected HTTP {expected_status}, got {} with \
                     {refused_body:?}",
                    refused.status
                );
                assert_eq!(
                    xml_element(&refused_body, "Code").as_deref(),
                    Some(expected_code),
                    "case {case:?}: expected <Code>{expected_code}</Code>, got {refused_body}"
                );
                assert!(
                    xml_element(&refused_body, "AccessKeyId").is_none(),
                    "case {case:?}: a refused exchange must vend no credential, got \
                     {refused_body}"
                );
            }

            // Redeeming is not consuming: the token still names the same identity, so the
            // same exchange is served again.
            let again =
                assume_with_web_identity(&router, &runtime, &role_arn, "second-session", &token);
            let again_body = again.text();
            assert_eq!(
                again.status, 200,
                "a token already exchanged must still be redeemable, got {} with \
                 {again_body:?}",
                again.status
            );
            assert_ne!(
                member("the second exchange", &again_body, "AccessKeyId"),
                access_key_id,
                "the second exchange must mint its own credential, got {again_body}"
            );
        })
        .expect("the sized thread spawns")
        .join()
        .expect("the suite runs to completion");
}
