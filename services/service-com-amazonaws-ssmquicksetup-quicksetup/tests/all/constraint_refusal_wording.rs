// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the sentence this service refuses a value outside a modelled
//! constraint with.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`, restJson1
//! serialization and the modelled-constraint walk — so what is asserted is what a signed
//! AWS SDK request observes.
//!
//! The rule under test: a value outside a modelled `@pattern` is refused with HTTP 400
//! `ValidationException` carrying the envelope
//! `1 validation error detected: Value '<value>' at '<member path>' failed to satisfy
//! constraint: Member must satisfy regular expression pattern: <pattern>` — and the
//! expression is quoted WITHOUT the `^`/`$` anchor pair the model wraps a whole-value
//! pattern in. The value, the member path and the expression all come from the request and
//! the model, so nothing about any one row is part of the rule.
//!
//! The rows span what it generalises over: which operation the request reaches, where the
//! refused value travels (a body member, a member nested inside a list element, an
//! `@httpLabel` path segment), and the shape of the expression the model declares (a
//! character class with a count, a dotted wildcard with a count, a class carrying an
//! escape).

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below signs for.
const REGION: &str = "us-west-2";

/// One emulator with an empty store, mounted exactly as the router mounts it. Every row
/// is refused at the modelled boundary, so none of them reaches a handler or the store.
fn emulator() -> MultiServiceRouter {
    let engine: std::sync::Arc<engine::InMemoryEngine> =
        std::sync::Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_ssmquicksetup_quicksetup::EmulatedService::new(engine),
    );
    router
}

/// A request signed for `ssm-quicksetup` in [`REGION`].
fn request(method: &str, uri: &str, body: Option<&Value>) -> http::Request<bytes::Bytes> {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("ssm-quicksetup.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDQUICKSETUP/20260101/{REGION}/ssm-quicksetup/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        );
    let payload = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            bytes::Bytes::from(serde_json::to_vec(value).expect("serialize the request body"))
        }
        None => bytes::Bytes::new(),
    };
    builder.body(payload).expect("a well-formed test request")
}

/// Status and parsed body of one call.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let response = router.dispatch(request(method, uri, body)).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let raw = response.into_body();
    let mut parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    if let (Some(error_type), Value::Object(map)) = (error_type, &mut parsed) {
        map.entry("__type")
            .or_insert_with(|| Value::String(error_type));
    }
    (status, parsed)
}

/// The error shape a failed call reported.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit('#')
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// The message a refusal carried, under either casing restJson1 may render an error
/// structure's single message member with.
fn error_message(body: &Value) -> String {
    ["Message", "message"]
        .iter()
        .filter_map(|key| body.get(*key))
        .filter_map(Value::as_str)
        .next()
        .unwrap_or("<no message>")
        .to_string()
}

/// Percent-encode a value for use as one `@httpLabel` path segment, which is what an SDK
/// does: every byte outside RFC 3986's unreserved set travels as `%XX`.
fn label(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(char::from(byte));
            }
            other => encoded.push_str(&format!("%{other:02X}")),
        }
    }
    encoded
}

/// One definition that breaks nothing: a published type deployed to an organizational
/// unit and a Region, so a row's own member is the only thing outside its constraint.
fn sound_definition() -> Value {
    let type_ = service_com_amazonaws_ssmquicksetup_quicksetup::state::QUICK_SETUP_TYPE_CATALOG[0]
        .type_;
    json!({
        "Type": type_,
        "Parameters": super::type_parameter_fixtures::with_required_parameters(
            type_,
            json!({ "TargetOrganizationalUnits": "ou-1", "TargetRegions": "us-east-1" }),
        ),
    })
}

/// One row: the request it sends, the member path the refusal must name, the value it must
/// quote, and the expression the MODEL declares for that member.
struct Row {
    case: &'static str,
    method: &'static str,
    uri: String,
    body: Option<Value>,
    /// The dotted member path, as a validation clause spells one.
    path: &'static str,
    /// The value the request carried, which the clause quotes.
    value: &'static str,
    /// The `@pattern` the model declares, anchors included
    /// (`smithy_query(query="operation", operation="<the row's operation>")`).
    modelled: &'static str,
}

/// Every row's rejected value is quoted in the envelope, at the member path the model
/// names, with the modelled expression stripped of the anchor pair that wraps it.
#[tokio::test]
async fn a_pattern_refusal_quotes_the_expression_without_its_anchors() {
    let router = emulator();
    let rows = vec![
        Row {
            case: "a body member of the create",
            method: "POST",
            uri: "/configurationManager".to_string(),
            body: Some(json!({
                "Name": "bad name!!",
                "ConfigurationDefinitions": [sound_definition()],
            })),
            path: "name",
            value: "bad name!!",
            modelled: "^[ A-Za-z0-9._-]{0,120}$",
        },
        Row {
            // `.` matches no line break, so a two-line description is outside a pattern
            // whose expression is a dotted wildcard rather than a character class.
            case: "a body member whose expression is a dotted wildcard",
            method: "POST",
            uri: "/configurationManager".to_string(),
            body: Some(json!({
                "Description": "line\nbreak",
                "ConfigurationDefinitions": [sound_definition()],
            })),
            path: "description",
            value: "line\nbreak",
            modelled: "^.{0,512}$",
        },
        Row {
            // A member inside a list ELEMENT: the clause names its position and the
            // collection step, and the expression carries an escape.
            case: "a member nested in a list element",
            method: "POST",
            uri: "/configurationManager".to_string(),
            body: Some(json!({
                "ConfigurationDefinitions": [{ "Type": "ab", "Parameters": {} }],
            })),
            path: "configurationDefinitions.1.member.type",
            value: "ab",
            modelled: r"^[a-zA-Z0-9_\-.:/]{3,200}$",
        },
        Row {
            case: "a second member of a list element",
            method: "POST",
            uri: "/configurationManager".to_string(),
            body: Some(json!({
                "ConfigurationDefinitions": [{
                    "Type": "AWSQuickSetupType-SSMHostMgmt",
                    "Parameters": {},
                    "LocalDeploymentExecutionRoleName": "not a role name!",
                }],
            })),
            path: "configurationDefinitions.1.member.localDeploymentExecutionRoleName",
            value: "not a role name!",
            modelled: r"^[\w+=,.@-]{1,64}$",
        },
        Row {
            // A value bound to the URI rather than to the body: the same envelope.
            case: "an httpLabel path segment",
            method: "GET",
            uri: format!("/getConfiguration/{}", label("bad id!")),
            body: None,
            path: "configurationId",
            value: "bad id!",
            modelled: "^[a-zA-Z0-9-_/:]{1,100}$",
        },
    ];

    for row in rows {
        let case = row.case;
        let (status, answer) = call(&router, row.method, &row.uri, row.body.as_ref()).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the modelled 400, got {status} / {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {answer:?}"
        );
        // The expression AWS quotes: the model's, less the one anchor pair wrapping it.
        let stated = row
            .modelled
            .strip_prefix('^')
            .and_then(|inner| inner.strip_suffix('$'))
            .unwrap_or_else(|| panic!("case {case:?}: the row's pattern must carry both anchors"));
        let expected = format!(
            "1 validation error detected: Value '{}' at '{}' failed to satisfy constraint: \
             Member must satisfy regular expression pattern: {stated}",
            row.value, row.path
        );
        let actual = error_message(&answer);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
        assert!(
            !actual.contains(row.modelled),
            "case {case:?}: the anchored expression must not be quoted, got {actual:?}"
        );
    }
}
