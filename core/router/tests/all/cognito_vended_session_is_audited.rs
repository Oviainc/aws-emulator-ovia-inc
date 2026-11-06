// SPDX-License-Identifier: CC0-1.0

//! The audit leg of a Cognito-vended session, across four services on one engine, and the
//! audit leg of the identity-pool administration that set it up.
//!
//! RULE: the credentials an identity pool vends are an `sts:AssumeRoleWithWebIdentity`
//! session of the pool's resolved role, so the assume is recorded in the account's
//! management-event history as STS's own `AssumeRoleWithWebIdentity` event — naming the
//! role it assumed, the pool it was assumed for and the identity it was assumed on behalf
//! of. The vend ITSELF is one of the identity data-plane calls, which CloudTrail delivers no
//! management event for, so it is in the history nowhere. A vend the role's trust policy does
//! NOT admit assumes nothing, so it records no assume at all either.
//!
//! RULE, the control-plane leg: a Cognito Identity control-plane call is a management
//! event of that service, so `cloudtrail:LookupEvents` reports it under the operation's
//! own `eventName` with `eventSource` `cognito-identity.amazonaws.com`, the members the
//! request carried, and the `readOnly` its kind implies.
//!
//! Only a cross-service suite can pin this: the role is written by `iam:CreateRole`, the
//! session is minted by the vend and resolved by `sts:GetCallerIdentity`, and the record is
//! read back through `cloudtrail:LookupEvents` — four services over ONE engine, as
//! `core/router/src/lib.rs` mounts them.
//!
//! Every call goes over the wire: the vend legs carry no credential scope at all (their
//! model lists no auth scheme), and the setup and lookup legs carry one, on a router with
//! request authentication off as every suite in this crate runs it.
//!
//! Assembling the whole router costs stack (see `boot_serve.rs`), so it is built — and the
//! requests driven — on a sized thread.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The Region every call below is signed for, and so the Region the pools, the identities
/// and the audit records live in.
const REGION: &str = "us-east-1";

/// The emulator's own address: an address, so the `Host` header names no service and the
/// unsigned vend legs carry their service name in `X-Amz-Target` alone.
const EMULATOR_HOST: &str = "127.0.0.1:9297";

/// The access key id every signed setup call carries.
const SETUP_KEY_ID: &str = "AKIDTEST";

/// The `RoleSessionName` every session an identity pool vends carries.
const SESSION_NAME: &str = "CognitoIdentityCredentials";

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

/// Dispatch one request, signed with `access_key_id` for `signing_name`, or unsigned when
/// `signing_name` is `None` — which is how an SDK sends an operation whose model lists no
/// auth scheme.
fn dispatch(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    signing_name: Option<&str>,
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
                 Credential={SETUP_KEY_ID}/20260101/{REGION}/{signing_name}/aws4_request, \
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
        &[
            ("content-type", "application/x-amz-json-1.1"),
            ("x-amz-target", target),
        ],
        bytes::Bytes::from(body.to_string()),
    )
}

/// One awsQuery call, which is how IAM is called.
fn query(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    signing_name: &str,
    body: String,
) -> Answer {
    dispatch(
        router,
        runtime,
        Some(signing_name),
        &[("content-type", "application/x-www-form-urlencoded")],
        bytes::Bytes::from(body),
    )
}

/// The text of the first `<name>…</name>` element of an XML body.
fn xml_element(body: &str, name: &str) -> String {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = body
        .find(&open)
        .unwrap_or_else(|| panic!("no <{name}> in {body}"))
        + open.len();
    let end = body[start..]
        .find(&close)
        .unwrap_or_else(|| panic!("no </{name}> in {body}"))
        + start;
    body[start..end].to_string()
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
        "iam",
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
    xml_element(&created.text(), "Arn")
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

/// `cognito-identity:SetIdentityPoolRoles` with one unauthenticated role.
fn set_unauthenticated_role(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    pool_id: &str,
    role_arn: &str,
) {
    let configured = rpc(
        router,
        runtime,
        Some("cognito-identity"),
        "AWSCognitoIdentityService.SetIdentityPoolRoles",
        &json!({"IdentityPoolId": pool_id, "Roles": {"unauthenticated": role_arn}}),
    );
    assert_eq!(
        configured.status,
        200,
        "setup: SetIdentityPoolRoles must succeed, got {} with {:?}",
        configured.status,
        configured.text()
    );
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

/// An unsigned `GetCredentialsForIdentity`.
fn vend(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    identity_id: &str,
) -> Answer {
    rpc(
        router,
        runtime,
        None,
        "AWSCognitoIdentityService.GetCredentialsForIdentity",
        &json!({"IdentityId": identity_id}),
    )
}

/// `cloudtrail:LookupEvents` filtered on one attribute, answering each event's
/// `CloudTrailEvent` document.
fn lookup(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    attribute_key: &str,
    attribute_value: &str,
) -> Vec<Value> {
    let answered = rpc(
        router,
        runtime,
        Some("cloudtrail"),
        "CloudTrail_20131101.LookupEvents",
        &json!({
            "LookupAttributes": [{
                "AttributeKey": attribute_key,
                "AttributeValue": attribute_value,
            }],
        }),
    );
    assert_eq!(
        answered.status,
        200,
        "LookupEvents({attribute_key}={attribute_value}) must succeed, got {} with {:?}",
        answered.status,
        answered.text()
    );
    answered.json()["Events"]
        .as_array()
        .unwrap_or_else(|| panic!("LookupEvents reports an Events list, got {}", answered.text()))
        .iter()
        .map(|event| {
            let document = event["CloudTrailEvent"]
                .as_str()
                .unwrap_or_else(|| panic!("an event carries its CloudTrailEvent: {event}"));
            serde_json::from_str(document)
                .unwrap_or_else(|error| panic!("CloudTrailEvent is JSON ({error}): {document}"))
        })
        .collect()
}

/// `cognito-identity:DescribeIdentityPool`, answering what it reported.
fn describe_pool(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    pool_id: &str,
) -> Answer {
    rpc(
        router,
        runtime,
        Some("cognito-identity"),
        "AWSCognitoIdentityService.DescribeIdentityPool",
        &json!({"IdentityPoolId": pool_id}),
    )
}

#[test]
fn a_vended_session_and_its_assume_are_auditable() {
    std::thread::Builder::new()
        .name("cognito-vended-session-audit".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a current-thread runtime");

            // The pool whose role admits its own users, and a second pool whose role
            // admits only the FIRST pool's users — the configuration that must vend
            // nothing.
            let served_pool = create_pool(&router, &runtime, "audited-vend");
            let refused_pool = create_pool(&router, &runtime, "audited-refusal");
            let served_role = create_role(
                &router,
                &runtime,
                "audited-vend-unauth",
                &trust_policy(&served_pool),
            );
            let other_pools_role = create_role(
                &router,
                &runtime,
                "audited-refusal-unauth",
                // Conditioned on the OTHER pool's id: this role admits the users of
                // `served_pool`, and it is the role `refused_pool` names.
                &trust_policy(&served_pool),
            );
            set_unauthenticated_role(&router, &runtime, &served_pool, &served_role);
            set_unauthenticated_role(&router, &runtime, &refused_pool, &other_pools_role);

            // The served leg: the vend answers a session of the pool's role.
            let served_identity = get_id(&router, &runtime, &served_pool);
            let served = vend(&router, &runtime, &served_identity);
            assert_eq!(
                served.status,
                200,
                "an unsigned vend from a pool whose role admits it must be served, got {} \
                 with {:?}",
                served.status,
                served.text()
            );
            let vended_key_id = served.json()["Credentials"]["AccessKeyId"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            assert!(
                !vended_key_id.is_empty(),
                "the vend must carry an AccessKeyId, got {:?}",
                served.text()
            );

            // The refused leg: the role trusts another pool's users, so nothing is vended.
            let refused_identity = get_id(&router, &runtime, &refused_pool);
            let refused = vend(&router, &runtime, &refused_identity);
            assert_eq!(
                refused.status,
                400,
                "a vend whose role trusts another pool must be refused, got {} with {:?}",
                refused.status,
                refused.text()
            );
            assert_eq!(
                refused.json()["__type"].as_str(),
                Some("InvalidIdentityPoolConfigurationException"),
                "expected InvalidIdentityPoolConfigurationException, got {:?}",
                refused.text()
            );
            assert_eq!(
                refused.json().get("Credentials"),
                None,
                "a refused vend carries no Credentials, got {:?}",
                refused.text()
            );

            // The assume the served vend made is auditable as STS's own event, found the
            // way a client finds it: a lookup by event name, and a lookup by event source.
            let expected_session_arn = format!(
                "arn:aws:sts::{}:assumed-role/audited-vend-unauth/{SESSION_NAME}",
                engine::emulator_account_id()
            );
            for (case, attribute_key, attribute_value) in [
                ("by event name", "EventName", "AssumeRoleWithWebIdentity"),
                ("by event source", "EventSource", "sts.amazonaws.com"),
                ("by the role it assumed", "ResourceName", served_role.as_str()),
            ] {
                let found = lookup(&router, &runtime, attribute_key, attribute_value);
                assert_eq!(
                    found.len(),
                    1,
                    "case {case:?}: exactly one assume was made, got {found:?}"
                );
                let event = &found[0];
                assert_eq!(
                    event.pointer("/responseElements/assumedRoleUser/arn"),
                    Some(&json!(expected_session_arn)),
                    "case {case:?}: the record names the session the vend answered, got \
                     {event}"
                );
                assert_eq!(
                    event.pointer("/responseElements/audience"),
                    Some(&json!(served_pool)),
                    "case {case:?}: the record names the pool the assume was for, got \
                     {event}"
                );
                assert_eq!(
                    event.pointer("/responseElements/subjectFromWebIdentityToken"),
                    Some(&json!(served_identity)),
                    "case {case:?}: the record names the identity, got {event}"
                );
                assert_eq!(
                    event.get("awsRegion"),
                    Some(&json!(REGION)),
                    "case {case:?}: the record is in the pool's Region, got {event}"
                );
                assert_eq!(
                    event.pointer("/userIdentity/type"),
                    Some(&json!("WebIdentityUser")),
                    "case {case:?}: the assume was made for a federated user, got {event}"
                );
                assert_eq!(
                    event.pointer("/userIdentity/identityProvider"),
                    Some(&json!("cognito-identity.amazonaws.com")),
                    "case {case:?}: the record names the provider that authenticated the \
                     user, got {event}"
                );
            }

            // Neither vend is in the management-event history: a vend is an identity
            // data-plane call, and CloudTrail delivers no management event for one — only the
            // assume it made is there.
            let vends = lookup(&router, &runtime, "EventName", "GetCredentialsForIdentity");
            assert!(
                vends.is_empty(),
                "a vend is a data event and reaches no management-event record, got {vends:?}"
            );

            // And no secret of the session reaches the log.
            let session_token = served.json()["Credentials"]["SessionToken"]
                .as_str()
                .expect("the vend carries a SessionToken")
                .to_string();
            let secret_key = served.json()["Credentials"]["SecretKey"]
                .as_str()
                .expect("the vend carries a SecretKey")
                .to_string();
            let recorded = lookup(&router, &runtime, "EventSource", "sts.amazonaws.com")
                .into_iter()
                .chain(lookup(
                    &router,
                    &runtime,
                    "EventName",
                    "GetCredentialsForIdentity",
                ))
                .map(|event| event.to_string())
                .collect::<Vec<String>>()
                .join("\n");
            for (case, secret) in [
                ("the session token", session_token),
                ("the secret key", secret_key),
            ] {
                assert!(
                    !recorded.contains(&secret),
                    "case {case:?}: no audit record may carry it, got {recorded}"
                );
            }
        })
        .expect("spawn the vended-session audit thread")
        .join()
        .expect("a vended session and its assume must be auditable");
}

/// RULE: a Cognito Identity CONTROL-PLANE call is a management event of this service, so
/// `cloudtrail:LookupEvents` reports it under the operation's own name with
/// `eventSource` `cognito-identity.amazonaws.com` and the members the request carried —
/// which is what lets a client audit identity-pool administration, and what lets a
/// pipeline use its own write as a liveness barrier before judging a later event's
/// absence.
///
/// Only a cross-service suite can pin this: the record is written by the service that
/// SERVES the call and read back through CloudTrail, two services over ONE engine.
#[test]
fn cognito_control_plane_calls_are_in_event_history() {
    std::thread::Builder::new()
        .name("cognito-control-plane-audit".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a current-thread runtime");

            let pool_name = "audited-control-plane";
            let pool_id = create_pool(&router, &runtime, pool_name);
            let described = describe_pool(&router, &runtime, &pool_id);
            assert_eq!(
                described.status,
                200,
                "the describe must be served, got {} with {:?}",
                described.status,
                described.text()
            );

            // The rows are the two control-plane calls that were made, each found the way
            // a client finds it — a lookup by event name — and each checked for the member
            // of ITS request that the record must carry.
            for (case, event_name, member, expected) in [
                (
                    "the create",
                    "CreateIdentityPool",
                    "identityPoolName",
                    pool_name.to_string(),
                ),
                (
                    "the describe",
                    "DescribeIdentityPool",
                    "identityPoolId",
                    pool_id.clone(),
                ),
            ] {
                let found = lookup(&router, &runtime, "EventName", event_name);
                assert_eq!(
                    found.len(),
                    1,
                    "case {case:?}: exactly one {event_name} was made, got {found:?}"
                );
                let event = &found[0];
                assert_eq!(
                    event.get("eventSource"),
                    Some(&json!("cognito-identity.amazonaws.com")),
                    "case {case:?}: the record names Cognito Identity as its source, got \
                     {event}"
                );
                assert_eq!(
                    event.pointer(&format!("/requestParameters/{member}")),
                    Some(&json!(expected)),
                    "case {case:?}: requestParameters.{member} must be {expected:?}, got \
                     {event}"
                );
                assert_eq!(
                    event.get("errorCode"),
                    None,
                    "case {case:?}: a served call carries no error code, got {event}"
                );
                assert_eq!(
                    event.get("awsRegion"),
                    Some(&json!(REGION)),
                    "case {case:?}: the record is in the Region the call was served in, \
                     got {event}"
                );
            }

            // The create CHANGED the account and the describe only reported it, which is
            // what `readOnly` tells apart and what a lookup can filter on.
            for (case, event_name, read_only) in [
                ("the create", "CreateIdentityPool", false),
                ("the describe", "DescribeIdentityPool", true),
            ] {
                let found = lookup(&router, &runtime, "ReadOnly", &read_only.to_string());
                let names: Vec<&str> = found
                    .iter()
                    .filter_map(|event| event.get("eventName").and_then(Value::as_str))
                    .collect();
                assert!(
                    names.contains(&event_name),
                    "case {case:?}: a lookup for readOnly={read_only} reports it, got \
                     {names:?}"
                );
            }
        })
        .expect("spawn the control-plane audit thread")
        .join()
        .expect("Cognito Identity control-plane calls must be in event history");
}
