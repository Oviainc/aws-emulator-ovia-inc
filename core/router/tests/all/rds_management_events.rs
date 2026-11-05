// SPDX-License-Identifier: CC0-1.0

//! Amazon RDS control-plane calls in CloudTrail event history, across two services on one
//! engine.
//!
//! RULE: a service that serves a control-plane API call records it as a management event
//! of that service, so `cloudtrail:LookupEvents` reports the call under its own
//! `eventName` with the serving service's `eventSource` — with the parameters the request
//! carried, every member the serving model declares `@sensitive` replaced by CloudTrail's
//! fixed redaction literal, and, for a failed call, the FAULT SHAPE's name as `errorCode`
//! (not always the code the caller saw: a query protocol renders the shape's
//! `awsQueryError` code on the wire).
//!
//! Only a cross-service suite can pin this: the record is written by the service that
//! SERVES the call (Amazon RDS, awsQuery) and read back through CloudTrail (awsJson1_1) —
//! two services over ONE engine, as `core/router/src/lib.rs` mounts them.
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

/// The Region every call below is signed for, and so the Region the records live in.
const REGION: &str = "us-west-2";

/// The access key id every call carries.
const ACCESS_KEY_ID: &str = "AKIDTEST";

/// The `eventSource` CloudTrail stamps on an Amazon RDS management event.
const RDS_EVENT_SOURCE: &str = "rds.amazonaws.com";

/// The literal CloudTrail stores in place of a sensitive request member's value.
const REDACTED: &str = "HIDDEN_DUE_TO_SECURITY_REASONS";

/// The master password the create below carries. Long enough for the member's modelled
/// constraints, and a value that appears nowhere else, so finding it in a record would be
/// unambiguous.
const MASTER_PASSWORD: &str = "n0t-in-the-trail";

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

/// Dispatch one request signed for `signing_name`.
fn dispatch(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
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
            // The signature is never checked: this router runs with authentication off.
            // What the scope carries is the key id and the Region a handler reads.
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential={ACCESS_KEY_ID}/20260101/{REGION}/{signing_name}/aws4_request, \
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

/// One awsQuery call to Amazon RDS.
fn rds(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    action: &str,
    params: &[(&str, &str)],
) -> Answer {
    let mut body = format!("Action={action}&Version=2014-10-31");
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
        "rds",
        &[("content-type", "application/x-www-form-urlencoded")],
        bytes::Bytes::from(body),
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
    let answered = dispatch(
        router,
        runtime,
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
fn rds_control_plane_calls_are_in_cloudtrail_event_history() {
    std::thread::Builder::new()
        .name("rds-management-events".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);
            // `enable_all`, which is what the emulator's own boot path builds: a DB
            // instance create opens the instance's engine listener through the core, and
            // binding a socket needs the runtime's I/O driver.
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("a current-thread runtime");

            // A create carrying a master password, and a read of an identifier nothing
            // holds — the two calls whose records this suite is about.
            let created = rds(
                &router,
                &runtime,
                "CreateDBInstance",
                &[
                    ("DBInstanceIdentifier", "trail-audited-db"),
                    ("Engine", "mysql"),
                    ("DBInstanceClass", "db.t3.medium"),
                    ("MasterUsername", "admin"),
                    ("MasterUserPassword", MASTER_PASSWORD),
                ],
            );
            assert_eq!(
                created.status,
                200,
                "the create must be served, got {} with {:?}",
                created.status,
                created.text()
            );
            let absent = rds(
                &router,
                &runtime,
                "DescribeDBInstances",
                &[("DBInstanceIdentifier", "no-such-db-instance")],
            );
            assert_eq!(
                absent.status,
                404,
                "a describe of an unknown identifier is a 404, got {} with {:?}",
                absent.status,
                absent.text()
            );
            // What the CALLER saw: the shape's `awsQueryError` code.
            let answered = xml_element(&absent.text(), "Code");
            assert_eq!(
                answered, "DBInstanceNotFound",
                "the caller sees the query error code, got {answered:?} in {:?}",
                absent.text()
            );

            // Both calls are in event history, found the way the check that filed this
            // issue finds them: a lookup by event source.
            let by_source = lookup(&router, &runtime, "EventSource", RDS_EVENT_SOURCE);
            assert_eq!(
                by_source.len(),
                2,
                "both RDS calls are recorded under {RDS_EVENT_SOURCE}, got {by_source:?}"
            );

            // The create's record: the sensitive member is the redaction literal, the
            // plaintext is nowhere, and the members that are not sensitive are as sent.
            let create = one_of("the create", &by_source, "CreateDBInstance");
            assert_eq!(
                create.pointer("/requestParameters/masterUserPassword"),
                Some(&json!(REDACTED)),
                "the master password must be withheld, got {create}"
            );
            assert_eq!(
                create.pointer("/requestParameters/dBInstanceIdentifier"),
                Some(&json!("trail-audited-db")),
                "the record names the instance the call created, got {create}"
            );
            assert_eq!(
                create.get("readOnly"),
                Some(&json!(false)),
                "a create changes the account, got {create}"
            );
            assert_eq!(
                create.get("errorCode"),
                None,
                "a served call carries no error code, got {create}"
            );

            // The failed read's record: the FAULT SHAPE's name, not the wire code.
            let refused = one_of("the refused read", &by_source, "DescribeDBInstances");
            assert_eq!(
                refused.get("errorCode"),
                Some(&json!("DBInstanceNotFoundFault")),
                "the trail records the fault shape name, got {refused}"
            );
            assert_eq!(
                refused.get("readOnly"),
                Some(&json!(true)),
                "a describe only reports state, got {refused}"
            );
            assert_eq!(
                refused.get("awsRegion"),
                Some(&json!(REGION)),
                "the record is in the Region the call was served in, got {refused}"
            );

            // No record of any of it carries the password the create sent.
            let whole_trail = by_source
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<String>>()
                .join("\n");
            assert!(
                !whole_trail.contains(MASTER_PASSWORD),
                "no record may carry the plaintext password, got {whole_trail}"
            );

            // And the same records are reachable by the other attributes a client looks
            // up with: the operation's name, and the resource the call addressed.
            for (case, attribute_key, attribute_value, event_name) in [
                (
                    "by event name",
                    "EventName",
                    "CreateDBInstance".to_string(),
                    "CreateDBInstance",
                ),
                (
                    "by the instance it addressed",
                    "ResourceName",
                    format!("arn:aws:rds:{REGION}:000000000000:db:trail-audited-db"),
                    "CreateDBInstance",
                ),
                (
                    "by resource type",
                    "ResourceType",
                    "AWS::RDS::DBInstance".to_string(),
                    "CreateDBInstance",
                ),
            ] {
                let found = lookup(&router, &runtime, attribute_key, &attribute_value);
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
        })
        .expect("spawn the RDS management-event thread")
        .join()
        .expect("Amazon RDS control-plane calls must be in CloudTrail event history");
}
