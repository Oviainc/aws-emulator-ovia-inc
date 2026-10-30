// SPDX-License-Identifier: CC0-1.0

//! IAM identity changes in CloudTrail event history, across two services on one engine.
//!
//! RULE: a service that serves a control-plane API call records it as a management event of
//! that service, so `cloudtrail:LookupEvents` reports the call under its own `eventName`
//! with the serving service's `eventSource` — and the credential material an IAM call
//! carries is recorded NOWHERE: the member holding it is absent from the recorded
//! `requestParameters` / `responseElements` entirely, not present with a placeholder value.
//! IAM is a global service, so its records are filed in one Region whatever Region the
//! caller signed for.
//!
//! Only a cross-service suite can pin this: the record is written by the service that
//! SERVES the call (IAM, awsQuery) and read back through CloudTrail (awsJson1_1) — two
//! services over ONE engine, as `core/router/src/lib.rs` mounts them.
//!
//! Every call goes over the wire, on a router with request authentication off as every
//! suite in this crate runs it. Assembling the whole router costs stack (see
//! `boot_serve.rs`), so it is built — and the requests driven — on a sized thread.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The Region IAM's event history lives in, and the Region a lookup for it is made in.
const LEADER_REGION: &str = "us-east-1";

/// A Region that is NOT the one IAM's records are filed in, for the lookup that must find
/// none of them.
const OTHER_REGION: &str = "eu-west-1";

/// The access key id every call carries.
const ACCESS_KEY_ID: &str = "AKIDTEST";

/// The `eventSource` CloudTrail stamps on an IAM management event.
const IAM_EVENT_SOURCE: &str = "iam.amazonaws.com";

/// The literal CloudTrail stores in place of a REDACTED member's value. An IAM record
/// carries it nowhere — the member is left out instead — so it is here to be asserted
/// absent.
const REDACTED: &str = "HIDDEN_DUE_TO_SECURITY_REASONS";

/// The console password the login-profile create below carries. A value that appears
/// nowhere else, so finding it in a record would be unambiguous.
const CONSOLE_PASSWORD: &str = "n0t-in-the-trail";

/// The IAM user the two audited calls are made for.
const USER_NAME: &str = "trail-audited-user";

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

/// Dispatch one request signed for `signing_name` in `region`.
fn dispatch(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    signing_name: &str,
    region: &str,
    headers: &[(&str, &str)],
    body: bytes::Bytes,
) -> Answer {
    let host = format!("{signing_name}.{region}.amazonaws.com");
    let mut builder = http::Request::builder()
        .method("POST")
        .uri(format!("https://{host}/"))
        .header("host", host.clone())
        .header(
            // The signature is never checked: this router runs with authentication off.
            // What the scope carries is the key id and the Region a handler reads.
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential={ACCESS_KEY_ID}/20260101/{region}/{signing_name}/aws4_request, \
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

/// One awsQuery call to IAM, made in `region`.
fn iam(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    region: &str,
    action: &str,
    params: &[(&str, &str)],
) -> Answer {
    let mut body = format!("Action={action}&Version=2010-05-08");
    for (name, value) in params {
        assert!(
            !value.contains(['&', '=', '+', '%']),
            "test parameter {name} would need form escaping: {value:?}"
        );
        body.push_str(&format!("&{name}={value}"));
    }
    dispatch(
        router,
        runtime,
        "iam",
        region,
        &[("content-type", "application/x-www-form-urlencoded")],
        bytes::Bytes::from(body),
    )
}

/// `cloudtrail:LookupEvents` in `region`, filtered on one attribute, answering each event's
/// `CloudTrailEvent` document.
fn lookup(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    region: &str,
    attribute_key: &str,
    attribute_value: &str,
) -> Vec<Value> {
    let answered = dispatch(
        router,
        runtime,
        "cloudtrail",
        region,
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

/// The text of the first `<name>` element of an XML body, or `""`.
fn xml_element(body: &str, name: &str) -> String {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    match body.find(&open) {
        None => String::new(),
        Some(at) => {
            let start = at + open.len();
            match body[start..].find(&close) {
                None => String::new(),
                Some(end) => body[start..start + end].to_string(),
            }
        }
    }
}

/// The one event of `event_name` among `events`.
fn one_of<'a>(case: &str, events: &'a [Value], event_name: &str) -> &'a Value {
    let found: Vec<&Value> = events
        .iter()
        .filter(|event| event.get("eventName") == Some(&json!(event_name)))
        .collect();
    assert_eq!(
        found.len(),
        1,
        "case {case:?}: expected exactly one {event_name} record, got {events:?}"
    );
    found[0]
}

#[test]
fn iam_identity_changes_are_in_cloudtrail_event_history_without_their_secrets() {
    std::thread::Builder::new()
        .name("iam-management-events".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a current-thread runtime");

            // The three calls of the reproduction: a user, a long-term access key for it
            // (whose response carries the secret), and a console password for the same user
            // (whose request carries the password). The key create is made in ANOTHER
            // Region, so the records' Region is this service's and not the caller's.
            let created = iam(
                &router,
                &runtime,
                LEADER_REGION,
                "CreateUser",
                &[("UserName", USER_NAME)],
            );
            assert_eq!(
                created.status,
                200,
                "the user create must be served, got {} with {:?}",
                created.status,
                created.text()
            );
            let minted = iam(
                &router,
                &runtime,
                OTHER_REGION,
                "CreateAccessKey",
                &[("UserName", USER_NAME)],
            );
            assert_eq!(
                minted.status,
                200,
                "the access-key create must be served, got {} with {:?}",
                minted.status,
                minted.text()
            );
            let secret = xml_element(&minted.text(), "SecretAccessKey");
            assert!(
                !secret.is_empty(),
                "the create answers the caller with a secret, got {:?}",
                minted.text()
            );
            let profiled = iam(
                &router,
                &runtime,
                LEADER_REGION,
                "CreateLoginProfile",
                &[
                    ("UserName", USER_NAME),
                    ("Password", CONSOLE_PASSWORD),
                    ("PasswordResetRequired", "true"),
                ],
            );
            assert_eq!(
                profiled.status,
                200,
                "the login-profile create must be served, got {} with {:?}",
                profiled.status,
                profiled.text()
            );

            // Every one of them is in event history, found in the leader Region the way a
            // client looking for IAM's events finds them: a lookup by event source.
            let by_source = lookup(
                &router,
                &runtime,
                LEADER_REGION,
                "EventSource",
                IAM_EVENT_SOURCE,
            );
            let names: Vec<&str> = by_source
                .iter()
                .filter_map(|event| event.get("eventName").and_then(Value::as_str))
                .collect();
            for expected in ["CreateUser", "CreateAccessKey", "CreateLoginProfile"] {
                assert!(
                    names.contains(&expected),
                    "{expected} is recorded under {IAM_EVENT_SOURCE}, got {names:?}"
                );
            }

            // The key create's record: everything the response carried EXCEPT the secret,
            // whose member is absent altogether.
            let key_create = one_of("the access-key create", &by_source, "CreateAccessKey");
            assert_eq!(
                key_create.pointer("/responseElements/accessKey/secretAccessKey"),
                None,
                "the record carries no secretAccessKey member at all, got {key_create}"
            );
            for (member, expected) in [("userName", json!(USER_NAME)), ("status", json!("Active"))] {
                assert_eq!(
                    key_create.pointer(&format!("/responseElements/accessKey/{member}")),
                    Some(&expected),
                    "the record carries the key's {member}, got {key_create}"
                );
            }
            assert!(
                key_create
                    .pointer("/responseElements/accessKey/accessKeyId")
                    .is_some_and(Value::is_string),
                "the record carries the key's id, got {key_create}"
            );
            assert!(
                key_create
                    .pointer("/responseElements/accessKey/createDate")
                    .is_some_and(Value::is_string),
                "the record carries the key's create date, got {key_create}"
            );
            assert_eq!(
                key_create.get("awsRegion"),
                Some(&json!(LEADER_REGION)),
                "a call signed for {OTHER_REGION} is still recorded in {LEADER_REGION}, got \
                 {key_create}"
            );
            assert_eq!(
                key_create.get("readOnly"),
                Some(&json!(false)),
                "minting a credential changes the account, got {key_create}"
            );

            // The login profile's record: the parameters the request carried EXCEPT the
            // password, whose member is likewise absent altogether.
            let profile = one_of("the login-profile create", &by_source, "CreateLoginProfile");
            assert_eq!(
                profile.pointer("/requestParameters/password"),
                None,
                "the record carries no password member at all, got {profile}"
            );
            assert_eq!(
                profile.pointer("/requestParameters/userName"),
                Some(&json!(USER_NAME)),
                "the record names the user the password was set for, got {profile}"
            );
            assert_eq!(
                profile.pointer("/requestParameters/passwordResetRequired"),
                Some(&json!(true)),
                "the record carries what the request asked for the reset, got {profile}"
            );

            // Neither the secret nor the password appears anywhere in event history, and
            // nothing stands in for them either.
            let whole_trail = by_source
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<String>>()
                .join("\n");
            for (case, withheld) in [
                ("the minted secret", secret.as_str()),
                ("the console password", CONSOLE_PASSWORD),
                ("a placeholder in place of either", REDACTED),
            ] {
                assert!(
                    !whole_trail.contains(withheld),
                    "case {case:?}: no record may carry it, got {whole_trail}"
                );
            }

            // The same records are reachable by the other attribute a client looks up
            // with: the operation's name.
            for (case, attribute_key, attribute_value, event_name) in [
                (
                    "by event name",
                    "EventName",
                    "CreateAccessKey".to_string(),
                    "CreateAccessKey",
                ),
                (
                    "by event name, for the login profile",
                    "EventName",
                    "CreateLoginProfile".to_string(),
                    "CreateLoginProfile",
                ),
                (
                    "by the user name the caller signed as",
                    "Username",
                    "emulator".to_string(),
                    "CreateUser",
                ),
            ] {
                let found = lookup(
                    &router,
                    &runtime,
                    LEADER_REGION,
                    attribute_key,
                    &attribute_value,
                );
                let names: Vec<&str> = found
                    .iter()
                    .filter_map(|event| event.get("eventName").and_then(Value::as_str))
                    .collect();
                assert!(
                    names.contains(&event_name),
                    "case {case:?}: a lookup for {attribute_key}={attribute_value} reports \
                     {event_name}, got {names:?}"
                );
            }

            // And a lookup in another Region reports none of them: IAM's event history
            // lives in the leader Region alone.
            let elsewhere = lookup(
                &router,
                &runtime,
                OTHER_REGION,
                "EventSource",
                IAM_EVENT_SOURCE,
            );
            assert!(
                elsewhere.is_empty(),
                "a lookup in {OTHER_REGION} reports no IAM record, got {elsewhere:?}"
            );
        })
        .expect("spawn the IAM management-event thread")
        .join()
        .expect("IAM identity changes must be in CloudTrail event history without their secrets");
}
