// SPDX-License-Identifier: CC0-1.0

//! STS calls in CloudTrail event history, across two services on one engine.
//!
//! RULE: a service that serves a management API call records it as a management event of
//! that service, so `cloudtrail:LookupEvents` reports the call under its own `eventName`
//! with the serving service's `eventSource` — and that holds for a READ-ONLY call as much
//! as for a write, which is how a client answers "who called this" for its own traffic.
//! The credential material an STS call mints is recorded NOWHERE: the member holding it is
//! absent from the recorded `responseElements` entirely, not present with a placeholder.
//!
//! Only a cross-service suite can pin this: the record is written by the service that
//! SERVES the call (STS, awsQuery) and read back through CloudTrail (awsJson1_1) — two
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

/// The Region the calls below are signed for, and the Region their records are looked up
/// in.
const REGION: &str = "us-east-1";

/// A Region none of the calls below was made in, for the lookup that must find none of
/// their records.
const OTHER_REGION: &str = "eu-west-1";

/// The access key id every call carries.
const ACCESS_KEY_ID: &str = "AKIDTEST";

/// The `eventSource` CloudTrail stamps on an STS management event.
const STS_EVENT_SOURCE: &str = "sts.amazonaws.com";

/// The literal CloudTrail stores in place of a REDACTED member's value. An STS record
/// carries it nowhere — the member is left out instead — so it is here to be asserted
/// absent.
const REDACTED: &str = "HIDDEN_DUE_TO_SECURITY_REASONS";

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

/// One awsQuery call to STS, made in `region`.
fn sts(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    region: &str,
    action: &str,
) -> Answer {
    dispatch(
        router,
        runtime,
        "sts",
        region,
        &[("content-type", "application/x-www-form-urlencoded")],
        bytes::Bytes::from(format!("Action={action}&Version=2011-06-15")),
    )
}

/// `cloudtrail:LookupEvents` in `region`, filtered on one attribute, answering each
/// reported event's envelope — the `Event` shape CloudTrail reports — paired with its
/// parsed `CloudTrailEvent` document.
fn lookup(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    region: &str,
    attribute_key: &str,
    attribute_value: &str,
) -> Vec<(Value, Value)> {
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
                "MaxResults": 50,
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
            let raw = event["CloudTrailEvent"]
                .as_str()
                .unwrap_or_else(|| panic!("an event carries its CloudTrailEvent: {event}"));
            let document: Value = serde_json::from_str(raw)
                .unwrap_or_else(|error| panic!("CloudTrailEvent is JSON ({error}): {raw}"));
            (event.clone(), document)
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

#[test]
fn sts_calls_are_in_cloudtrail_event_history_without_their_secrets() {
    std::thread::Builder::new()
        .name("sts-management-events".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a current-thread runtime");

            // (case, the action, whether the call only REPORTS state)
            let rows: [(&str, &str, bool); 2] = [
                // The read a client makes to answer "who am I" — the call whose own audit
                // record it must be able to look up.
                ("a read", "GetCallerIdentity", true),
                // A call that MINTS a session: a write, whose response carries credential
                // material.
                ("a mint", "GetSessionToken", false),
            ];

            let mut minted_secrets: Vec<(&str, String)> = Vec::new();
            for (case, action, read_only) in rows {
                let answered = sts(&router, &runtime, REGION, action);
                assert_eq!(
                    answered.status,
                    200,
                    "case {case:?}: {action} must be served, got {} with {:?}",
                    answered.status,
                    answered.text()
                );
                for member in ["SecretAccessKey", "SessionToken"] {
                    let value = xml_element(&answered.text(), member);
                    if !value.is_empty() {
                        minted_secrets.push((member, value));
                    }
                }

                // Found the way a client finds the record of its own call: a lookup by
                // event name.
                let found = lookup(&router, &runtime, REGION, "EventName", action);
                assert_eq!(
                    found.len(),
                    1,
                    "case {case:?}: exactly one {action} was made, got {found:?}"
                );
                let (envelope, document) = &found[0];
                assert_eq!(
                    (
                        envelope.get("EventName"),
                        envelope.get("EventSource"),
                        envelope.get("ReadOnly"),
                    ),
                    (
                        Some(&json!(action)),
                        Some(&json!(STS_EVENT_SOURCE)),
                        Some(&json!(read_only.to_string())),
                    ),
                    "case {case:?}: expected the call's own envelope, got {envelope}"
                );
                assert_eq!(
                    (
                        document.get("eventName"),
                        document.get("eventSource"),
                        document.get("readOnly"),
                        document.get("awsRegion"),
                        document.get("eventType"),
                    ),
                    (
                        Some(&json!(action)),
                        Some(&json!(STS_EVENT_SOURCE)),
                        Some(&json!(read_only)),
                        Some(&json!(REGION)),
                        Some(&json!("AwsApiCall")),
                    ),
                    "case {case:?}: expected the call's own document, got {document}"
                );
                assert_eq!(
                    document.get("errorCode"),
                    None,
                    "case {case:?}: a served call carries no error code, got {document}"
                );
                assert_eq!(
                    document.pointer("/userIdentity/accessKeyId"),
                    Some(&json!(ACCESS_KEY_ID)),
                    "case {case:?}: the record names the credential the call was signed \
                     with, got {document}"
                );
                // A read answers with nothing a record carries; a mint's response IS
                // recorded, minus its credential material.
                if read_only {
                    assert_eq!(
                        document.get("responseElements"),
                        Some(&Value::Null),
                        "case {case:?}: a read is recorded with no response elements, got \
                         {document}"
                    );
                }
                for member in ["secretAccessKey", "sessionToken"] {
                    assert_eq!(
                        document.pointer(&format!("/responseElements/credentials/{member}")),
                        None,
                        "case {case:?}: the record carries no {member} member at all, got \
                         {document}"
                    );
                }
            }

            // The mint answered its caller with credential material, and none of it is in
            // any record — neither the value nor a placeholder standing for it.
            assert!(
                !minted_secrets.is_empty(),
                "the mint must answer its caller with credential material, or this case \
                 proves nothing"
            );
            let recorded = lookup(&router, &runtime, REGION, "EventSource", STS_EVENT_SOURCE)
                .iter()
                .map(|(envelope, document)| format!("{envelope}{document}"))
                .collect::<Vec<String>>()
                .join("\n");
            for (member, secret) in &minted_secrets {
                assert!(
                    !recorded.contains(secret.as_str()),
                    "case {member:?}: no audit record may carry it, got {recorded}"
                );
            }
            assert!(
                !recorded.contains(REDACTED),
                "an STS record withholds a value by omission, never with a placeholder, \
                 got {recorded}"
            );

            // The records are filed in the Region the calls were signed for: a lookup in
            // another Region reports none of them.
            let elsewhere = lookup(
                &router,
                &runtime,
                OTHER_REGION,
                "EventSource",
                STS_EVENT_SOURCE,
            );
            assert!(
                elsewhere.is_empty(),
                "case another Region: a lookup there reports no event of a call made in \
                 {REGION}, got {elsewhere:?}"
            );
        })
        .expect("spawn the sts-management-events thread")
        .join()
        .expect("STS calls must be in CloudTrail event history");
}
