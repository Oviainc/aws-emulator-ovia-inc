// SPDX-License-Identifier: CC0-1.0

//! Source identity, end to end: `sts:AssumeRole` records the value it was given, and every
//! call the resulting session makes carries it in the record's `sessionContext`.
//!
//! RULE: a session minted by STS is part of the account's audit record. The assume itself is
//! a management event of `sts.amazonaws.com` carrying `requestParameters.roleArn`,
//! `roleSessionName` and `sourceIdentity` with `responseElements.assumedRoleUser.arn`; and a
//! call made WITH the session's credentials — whichever service serves it — is recorded as
//! that session: `userIdentity.type` `AssumedRole`, `sessionContext.sessionIssuer.arn` the
//! ROLE's ARN, and `sessionContext.sourceIdentity` the value the session carries. The value
//! persists across chained role sessions ("The source identity value persists across chained
//! role sessions", `AssumeRoleRequest$SourceIdentity`), and a session assumed WITHOUT one
//! carries no `sourceIdentity` at all — the twin that separates propagation from stamping a
//! value on everything.
//!
//! Only a cross-service suite can pin this: the role is written by `iam:CreateRole`, the
//! session is minted by `sts:AssumeRole`, the session's own call is served by a THIRD
//! service, and every record is read back through `cloudtrail:LookupEvents` — four services
//! over ONE engine, as `core/router/src/lib.rs` mounts them.
//!
//! Every call goes over the wire, on a router with request authentication off as every suite
//! in this crate runs it: what a credential scope carries is the key id a handler reads the
//! caller from.
//!
//! Assembling the whole router costs stack (see `boot_serve.rs`), so it is built — and the
//! requests driven — on a sized thread.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The Region every call below is signed for.
const REGION: &str = "us-east-1";

/// The access key id the setup calls carry: a long-term key, which is what makes the
/// recorded caller of the FIRST assume an IAM user rather than a session.
const SETUP_KEY_ID: &str = "AKIDTEST";

/// The STS API version every `awsQuery` call to STS carries.
const STS_API_VERSION: &str = "2011-06-15";

/// The IAM API version every `awsQuery` call to IAM carries.
const IAM_API_VERSION: &str = "2010-05-08";

/// The Elastic Load Balancing API version, for the third-service call a session makes.
const ELB_API_VERSION: &str = "2015-12-01";

/// The source identity the first assume sets: inside `sourceIdentityType` (length 2..64,
/// pattern `^[\w+=,.@-]*$`).
const SOURCE_IDENTITY: &str = "simparity-analyst@example.com";

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

/// Dispatch one request signed (in the routing sense) with `access_key_id` for
/// `signing_name`.
fn dispatch(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    access_key_id: &str,
    signing_name: &str,
    headers: &[(&str, &str)],
    body: bytes::Bytes,
) -> Answer {
    let host = format!("{signing_name}.{REGION}.amazonaws.com");
    let mut builder = http::Request::builder()
        .method("POST")
        .uri(format!("https://{host}/"))
        .header("host", host.clone())
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential={access_key_id}/20260101/{REGION}/{signing_name}/aws4_request, \
                 SignedHeaders=host, Signature=not-verified"
            ),
        );
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

/// Percent-encode what is significant in an `application/x-www-form-urlencoded` body.
fn form_value(value: &str) -> String {
    value
        .chars()
        .map(|character| match character {
            '&' => "%26".to_string(),
            '=' => "%3D".to_string(),
            '+' => "%2B".to_string(),
            '%' => "%25".to_string(),
            ' ' => "%20".to_string(),
            other => other.to_string(),
        })
        .collect()
}

/// One awsQuery call against `signing_name`, signed with `access_key_id`.
fn query(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    access_key_id: &str,
    signing_name: &str,
    version: &str,
    action: &str,
    params: &[(&str, &str)],
) -> Answer {
    let mut body = format!("Action={action}&Version={version}");
    for (name, value) in params {
        body.push('&');
        body.push_str(name);
        body.push('=');
        body.push_str(&form_value(value));
    }
    dispatch(
        router,
        runtime,
        access_key_id,
        signing_name,
        &[("content-type", "application/x-www-form-urlencoded")],
        bytes::Bytes::from(body),
    )
}

/// The text of the first `<name>…</name>` element of an XML body.
fn xml_element(body: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)? + start;
    Some(body[start..end].to_string())
}

/// A role this account's own principals can assume, carry a source identity into, and whose
/// sessions may chain into it again; answers its ARN.
fn create_role(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    role_name: &str,
) -> String {
    let document = format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
         \"Principal\":{{\"AWS\":\"arn:aws:iam::{}:root\"}},\
         \"Action\":[\"sts:AssumeRole\",\"sts:SetSourceIdentity\"]}}]}}",
        engine::emulator_account_id()
    );
    let created = query(
        router,
        runtime,
        SETUP_KEY_ID,
        "iam",
        IAM_API_VERSION,
        "CreateRole",
        &[
            ("RoleName", role_name),
            ("AssumeRolePolicyDocument", &document),
        ],
    );
    assert_eq!(
        created.status,
        200,
        "setup: CreateRole({role_name:?}) must succeed, got {} with {:?}",
        created.status,
        created.text()
    );
    xml_element(&created.text(), "Arn").expect("CreateRole reports the role ARN")
}

/// `sts:AssumeRole` of `role_arn`, answering the access key id of the session it minted.
fn assume_role(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    caller_key: &str,
    role_arn: &str,
    session_name: &str,
    source_identity: Option<&str>,
) -> String {
    let mut params = vec![("RoleArn", role_arn), ("RoleSessionName", session_name)];
    if let Some(source_identity) = source_identity {
        params.push(("SourceIdentity", source_identity));
    }
    let assumed = query(
        router,
        runtime,
        caller_key,
        "sts",
        STS_API_VERSION,
        "AssumeRole",
        &params,
    );
    assert_eq!(
        assumed.status,
        200,
        "setup: AssumeRole({session_name:?}) must succeed, got {} with {:?}",
        assumed.status,
        assumed.text()
    );
    xml_element(&assumed.text(), "AccessKeyId").expect("the assume answers an AccessKeyId")
}

/// One management call made with `access_key_id` to a service that is NEITHER the one that
/// minted the session nor the one that reports the record.
fn session_call(router: &MultiServiceRouter, runtime: &tokio::runtime::Runtime, access_key_id: &str) {
    let described = query(
        router,
        runtime,
        access_key_id,
        "elasticloadbalancingv2",
        ELB_API_VERSION,
        "DescribeLoadBalancers",
        &[],
    );
    assert_eq!(
        described.status,
        200,
        "the session's own service call must be served, got {} with {:?}",
        described.status,
        described.text()
    );
}

/// `cloudtrail:LookupEvents` filtered on one attribute, answering each event's
/// `CloudTrailEvent` document, most recent first.
fn lookup(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    attribute_key: &str,
    attribute_value: &str,
) -> Vec<Value> {
    let answered = dispatch(
        router,
        runtime,
        SETUP_KEY_ID,
        "cloudtrail",
        &[
            ("content-type", "application/x-amz-json-1.1"),
            ("x-amz-target", "CloudTrail_20131101.LookupEvents"),
        ],
        bytes::Bytes::from(
            json!({
                "LookupAttributes": [{
                    "AttributeKey": attribute_key,
                    "AttributeValue": attribute_value,
                }],
            })
            .to_string(),
        ),
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

/// A string member of an event document.
fn at(event: &Value, pointer: &str) -> Option<String> {
    event
        .pointer(pointer)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

/// The record of the assume that named `session_name`.
fn assume_record(events: &[Value], session_name: &str) -> Value {
    events
        .iter()
        .find(|event| {
            at(event, "/requestParameters/roleSessionName").as_deref() == Some(session_name)
        })
        .unwrap_or_else(|| panic!("no AssumeRole record for session {session_name:?}: {events:?}"))
        .clone()
}

#[test]
fn source_identity_propagates_from_the_assume_into_the_sessions_own_records() {
    std::thread::Builder::new()
        .name("sts-source-identity-audit".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a current-thread runtime");

            let role_arn = create_role(&router, &runtime, "source-identity-audit");
            let account = engine::emulator_account_id().to_string();

            // The three sessions the rows read: one assumed WITH a source identity, one
            // CHAINED from it supplying none of its own, and the twin control assumed
            // without one at all.
            let identified = assume_role(
                &router,
                &runtime,
                SETUP_KEY_ID,
                &role_arn,
                "identified",
                Some(SOURCE_IDENTITY),
            );
            let chained =
                assume_role(&router, &runtime, &identified, &role_arn, "chained", None);
            let anonymous =
                assume_role(&router, &runtime, SETUP_KEY_ID, &role_arn, "anonymous", None);
            for credential in [&identified, &chained, &anonymous] {
                session_call(&router, &runtime, credential);
            }

            // The assume itself is in event history, with the members the IAM User Guide
            // says to look up ("You can find the sourceIdentity information in the
            // requestParameters key in your CloudTrail log", alongside roleArn and
            // roleSessionName).
            let assumes = lookup(&router, &runtime, "EventName", "AssumeRole");
            assert_eq!(
                assumes.len(),
                3,
                "three assumes were made, got {assumes:?}"
            );
            // (case, the session the assume named, the sourceIdentity its REQUEST carried)
            for (case, session_name, requested) in [
                ("the assume that set one", "identified", Some(SOURCE_IDENTITY)),
                ("the chained assume", "chained", None),
                ("the assume that set none", "anonymous", None),
            ] {
                let record = assume_record(&assumes, session_name);
                assert_eq!(
                    at(&record, "/eventSource").as_deref(),
                    Some("sts.amazonaws.com"),
                    "case {case:?}: the assume is STS's own event, got {record}"
                );
                assert_eq!(
                    at(&record, "/requestParameters/roleArn").as_deref(),
                    Some(role_arn.as_str()),
                    "case {case:?}: the record names the role, got {record}"
                );
                assert_eq!(
                    at(&record, "/requestParameters/sourceIdentity").as_deref(),
                    requested,
                    "case {case:?}: expected requestParameters.sourceIdentity {requested:?}, \
                     got {record}"
                );
                assert_eq!(
                    at(&record, "/responseElements/assumedRoleUser/arn").as_deref(),
                    Some(
                        format!(
                            "arn:aws:sts::{account}:assumed-role/source-identity-audit/\
                             {session_name}"
                        )
                        .as_str()
                    ),
                    "case {case:?}: the record names the session it answered, got {record}"
                );
            }

            // And every call the sessions made carries the session in its own record: the
            // issuing role, and the source identity only where the session has one.
            let session_calls = lookup(&router, &runtime, "EventName", "DescribeLoadBalancers");
            assert_eq!(
                session_calls.len(),
                3,
                "each session made one call, got {session_calls:?}"
            );
            // (case, the credential the call was made with, the session's name, the
            // sourceIdentity the record must carry)
            for (case, credential, session_name, source_identity) in [
                (
                    "the session that set a source identity",
                    identified.as_str(),
                    "identified",
                    Some(SOURCE_IDENTITY),
                ),
                (
                    "the chained session, which set none of its own",
                    chained.as_str(),
                    "chained",
                    Some(SOURCE_IDENTITY),
                ),
                (
                    "the session assumed without one",
                    anonymous.as_str(),
                    "anonymous",
                    None,
                ),
            ] {
                let record = session_calls
                    .iter()
                    .find(|event| {
                        at(event, "/userIdentity/accessKeyId").as_deref() == Some(credential)
                    })
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: no record of the call: {session_calls:?}")
                    });
                assert_eq!(
                    at(record, "/userIdentity/type").as_deref(),
                    Some("AssumedRole"),
                    "case {case:?}: a session call is made by an assumed role, got {record}"
                );
                assert_eq!(
                    at(record, "/userIdentity/arn").as_deref(),
                    Some(
                        format!(
                            "arn:aws:sts::{account}:assumed-role/source-identity-audit/\
                             {session_name}"
                        )
                        .as_str()
                    ),
                    "case {case:?}: the record names the session, got {record}"
                );
                assert_eq!(
                    at(record, "/userIdentity/sessionContext/sessionIssuer/arn").as_deref(),
                    Some(role_arn.as_str()),
                    "case {case:?}: the issuer is the ROLE, got {record}"
                );
                assert_eq!(
                    at(record, "/userIdentity/sessionContext/sourceIdentity").as_deref(),
                    source_identity,
                    "case {case:?}: expected sessionContext.sourceIdentity \
                     {source_identity:?}, got {record}"
                );
            }
        })
        .expect("spawn the source-identity audit thread")
        .join()
        .expect("source identity must propagate into the session's own audit records");
}
