// SPDX-License-Identifier: CC0-1.0

//! MediaConnect's client-error envelopes, end to end over HTTP, and the rule that a flow
//! ARN carried as an `@httpLabel` is an IDENTIFIER the store resolves rather than a member
//! the boundary judges.
//!
//! Two rules are pinned here, each over a table that spans its class rather than the one
//! request that exposed it:
//!
//! 1. A flow operation's `{FlowArn}` label is handed to the handler whatever its form, and
//!    a label the store does not hold is refused `NotFoundException` (404) — never
//!    `BadRequestException` (400) for failing the `FlowArn` SHAPE's pattern, which is
//!    narrower than the pattern each of those members declares for itself.
//! 2. `NotFoundException$Message` is `Not found: <the identifier the request named>`, and
//!    `BadRequestException$Message` is `Validation failed:`, a newline, then what was
//!    unacceptable — whether the refusal came from the framework's constraint walk or from
//!    a handler.
//!
//! Every case drives the real request path (routing, restJson1 deserialization, constraint
//! validation, handler, serialization) through the harness
//! [`crate::flow_lifecycle::Emulator`], so what they assert is what a MediaConnect client
//! sees on the wire.

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::flow_lifecycle::{Emulator, REGION};

/// An account id used in the ARNs below that address nothing.
const ACCOUNT: &str = "111122223333";

/// An Amazon Web Services account id a `GrantEntitlementRequest` names as a subscriber.
const SUBSCRIBER: &str = "444455556666";

/// One call of a flow operation addressed by a `{FlowArn}` label: the HTTP binding plus a
/// body that is valid for it, so the only thing a refusal can be about is the label.
struct FlowCall {
    operation: &'static str,
    method: &'static str,
    /// The operation's URI with `{}` where the label goes.
    uri: &'static str,
    body: Option<Value>,
}

/// Every flow operation this crate implements that takes the flow's ARN as an
/// `@httpLabel`, with a request body the model accepts
/// (`smithy_query(query="operation",operation="…")` for each binding). A body that would
/// itself be refused would hide what these cases are about.
fn flow_calls() -> Vec<FlowCall> {
    vec![
        FlowCall {
            operation: "DescribeFlow",
            method: "GET",
            uri: "/v1/flows/{}",
            body: None,
        },
        FlowCall {
            operation: "DeleteFlow",
            method: "DELETE",
            uri: "/v1/flows/{}",
            body: None,
        },
        FlowCall {
            operation: "StartFlow",
            method: "POST",
            uri: "/v1/flows/start/{}",
            body: None,
        },
        FlowCall {
            operation: "StopFlow",
            method: "POST",
            uri: "/v1/flows/stop/{}",
            body: None,
        },
        FlowCall {
            operation: "UpdateFlow",
            method: "PUT",
            uri: "/v1/flows/{}",
            body: Some(json!({ "flowSize": "MEDIUM" })),
        },
        FlowCall {
            operation: "AddFlowOutputs",
            method: "POST",
            uri: "/v1/flows/{}/outputs",
            body: Some(json!({ "outputs": [
                { "name": "out", "protocol": "rtp", "destination": "203.0.113.5", "port": 5000 }
            ] })),
        },
        FlowCall {
            operation: "AddFlowMediaStreams",
            method: "POST",
            uri: "/v1/flows/{}/mediaStreams",
            body: Some(json!({ "mediaStreams": [
                { "mediaStreamId": 1, "mediaStreamName": "vid", "mediaStreamType": "video" }
            ] })),
        },
        FlowCall {
            operation: "GrantFlowEntitlements",
            method: "POST",
            uri: "/v1/flows/{}/entitlements",
            body: Some(json!({ "entitlements": [
                { "name": "ent", "subscribers": [SUBSCRIBER] }
            ] })),
        },
    ]
}

/// The forms a `{FlowArn}` label can take that the `FlowArn` SHAPE's pattern
/// (`smithy_query(query="shape",shape="FlowArn")`:
/// `^arn:(aws[a-zA-Z-]*):mediaconnect:[a-z0-9-]+:[0-9]{12}:flow:[a-zA-Z0-9-]+:[a-zA-Z0-9_-]+$`)
/// refuses, while the pattern each flow member declares for itself
/// (`^arn:.+:mediaconnect.+:flow:.+$`) admits all but the first — each paired with the
/// identifier the service's not-found envelope names for it. The rows differ along what the
/// rule generalises over: not an ARN at all, then one segment of the ARN at a time —
/// partition, account, resource id, resource name.
///
/// A label the member's own pattern admits reached the flow lookup and resolved nothing, so
/// the envelope names that ARN; a label it does not admit is no flow ARN, so it reached no
/// lookup and the envelope carries the generic reason instead of the caller's text.
fn labels_outside_the_shape_pattern() -> Vec<(&'static str, String, String)> {
    let generic = mc::UNRESOLVED_PATH_NOT_FOUND_REASON.to_string();
    vec![
        (
            "not an ARN at all",
            "not-a-valid-flow-arn".to_string(),
            generic,
        ),
        (
            "another partition, and an account segment that is not twelve digits",
            "arn:aws-cn:mediaconnect:cn-north-1:acct:flow:1-00000001:f1".to_string(),
            "arn:aws-cn:mediaconnect:cn-north-1:acct:flow:1-00000001:f1".to_string(),
        ),
        (
            "an eleven-digit account id",
            format!("arn:aws:mediaconnect:{REGION}:11112222333:flow:1-0000beef:f"),
            format!("arn:aws:mediaconnect:{REGION}:11112222333:flow:1-0000beef:f"),
        ),
        (
            "a flow id outside the shape's [a-zA-Z0-9-]+ class",
            format!("arn:aws:mediaconnect:{REGION}:{ACCOUNT}:flow:1_0000beef:f"),
            format!("arn:aws:mediaconnect:{REGION}:{ACCOUNT}:flow:1_0000beef:f"),
        ),
        (
            "a flow name outside the shape's [a-zA-Z0-9_-]+ class",
            format!("arn:aws:mediaconnect:{REGION}:{ACCOUNT}:flow:1-0000beef:f.1"),
            format!("arn:aws:mediaconnect:{REGION}:{ACCOUNT}:flow:1-0000beef:f.1"),
        ),
        (
            "an ARN of the shape's own form that names no flow",
            format!("arn:aws:mediaconnect:{REGION}:{ACCOUNT}:flow:1-0000dead:never-minted"),
            format!("arn:aws:mediaconnect:{REGION}:{ACCOUNT}:flow:1-0000dead:never-minted"),
        ),
    ]
}

/// Create one flow and return `(FlowArn, Name)`.
async fn create_flow(emulator: &Emulator, case: &str, name: &str) -> String {
    let (status, code, created) = emulator
        .create_flow(json!({
            "name": name,
            "source": { "name": "src", "protocol": "rtp", "whitelistCidr": "10.0.0.0/16" }
        }))
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "case {case:?}: the fixture flow must be created, got {status} {code:?}: {created}"
    );
    created["flow"]["flowArn"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// A flow ARN label resolves against the STORE, whatever its form: every flow operation
/// answers `NotFoundException` (404) for a label it does not hold — including labels the
/// `FlowArn` shape's narrower pattern refuses but each member's own pattern admits, and a
/// label that is no ARN at all — and the message is that label in this service's not-found
/// envelope when the label is a flow ARN, and the generic reason when it is not.
#[tokio::test]
async fn a_flow_arn_label_is_resolved_by_the_store_whatever_its_form() {
    let emulator = Emulator::new();

    for (case, label, identifier) in labels_outside_the_shape_pattern() {
        for FlowCall {
            operation,
            method,
            uri,
            body,
        } in flow_calls()
        {
            let uri = uri.replace("{}", &label);
            let (status, code, response) = emulator.send(method, &uri, body).await;
            assert_eq!(
                (status, code.as_deref()),
                (404, Some("NotFoundException")),
                "case {case:?} / {operation}: a label the store does not hold is 404 \
                 NotFoundException, got {status} {code:?}: {response}"
            );
            assert_eq!(
                response["__type"].as_str(),
                Some("NotFoundException"),
                "case {case:?} / {operation}: the body names the modelled shape, got {response}"
            );
            assert_eq!(
                response["message"].as_str(),
                Some(mc::not_found_message(&identifier).as_str()),
                "case {case:?} / {operation}: expected {:?}, got {response}",
                mc::not_found_message(&identifier)
            );
        }
    }

    // The positive control for the same rule: a label the store DOES hold is served, so
    // handing the label to the handler unjudged did not stop the label from resolving.
    let flow_arn = create_flow(&emulator, "positive control", "resolvable-flow").await;
    let (status, code, described) = emulator.describe_flow(&flow_arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "a label the store holds is served, got {status} {code:?}: {described}"
    );
    assert_eq!(
        described["flow"]["flowArn"].as_str(),
        Some(flow_arn.as_str()),
        "the served flow is the one the label named, got {described}"
    );
}

/// One hostile `{FlowArn}` label: what it is made of, the label as it travels in the
/// request line (percent-encoded where the URI grammar requires it), and the identifier the
/// service's not-found envelope names for it.
struct HostileLabel {
    case: &'static str,
    /// The label as written in the request line.
    encoded: String,
    /// The identifier the envelope names: the decoded label when the label is a flow ARN,
    /// the generic reason when it is not, or `None` when the test asserts only the
    /// envelope's prefix.
    identifier: Option<String>,
}

/// Handing the `{FlowArn}` label to the handler unjudged (Cause 1) means an arbitrary
/// caller-supplied byte string reaches handler code, so every consumer of that value must
/// be TOTAL: the store lookup (a `BTreeMap::get`), the message envelope
/// (`crate::state::flow_not_found_message`, a pattern walk and one `format!`) and the
/// restJson1 error serializer. This drives labels built to break a consumer that parses,
/// slices or indexes — a long one, multi-byte text, quotes and braces and backslashes, a
/// traversal attempt, an ARN-shaped label with an empty last segment — and asserts each is
/// answered, not crashed: 404 `NotFoundException` whose message is the service's envelope.
#[tokio::test]
async fn a_hostile_flow_arn_label_is_answered_not_crashed() {
    let emulator = Emulator::new();
    let long_name = "n".repeat(2_000);
    // A label that is no flow ARN reached no flow lookup, so the envelope carries the
    // generic reason rather than the caller's own bytes.
    let generic = mc::UNRESOLVED_PATH_NOT_FOUND_REASON.to_string();

    let labels = vec![
        HostileLabel {
            case: "a two-thousand-character label",
            encoded: long_name.clone(),
            identifier: Some(generic.clone()),
        },
        HostileLabel {
            case: "multi-byte text, percent-encoded in the request line",
            // "流-名前" — three-byte code points, so a consumer that indexed bytes would
            // split a character.
            encoded: "%E6%B5%81-%E5%90%8D%E5%89%8D".to_string(),
            identifier: Some(generic.clone()),
        },
        HostileLabel {
            case: "quotes, braces and a backslash, which the JSON envelope must escape",
            encoded: "a%22b%5Cc%7Bd%7D".to_string(),
            identifier: Some(generic.clone()),
        },
        HostileLabel {
            case: "a path-traversal attempt, percent-encoded so it stays one label",
            encoded: "..%2F..%2Fetc%2Fpasswd".to_string(),
            identifier: Some(generic.clone()),
        },
        HostileLabel {
            case: "an ARN-shaped label whose last segment is empty",
            encoded: format!("arn:aws:mediaconnect:{REGION}:{ACCOUNT}:flow:1-0000beef:"),
            identifier: Some(format!(
                "arn:aws:mediaconnect:{REGION}:{ACCOUNT}:flow:1-0000beef:"
            )),
        },
        HostileLabel {
            case: "a label that is one colon",
            encoded: "%3A".to_string(),
            identifier: Some(generic.clone()),
        },
    ];

    for HostileLabel {
        case,
        encoded,
        identifier,
    } in labels
    {
        for (operation, method, uri) in [
            ("DescribeFlow", "GET", format!("/v1/flows/{encoded}")),
            ("DeleteFlow", "DELETE", format!("/v1/flows/{encoded}")),
            ("StartFlow", "POST", format!("/v1/flows/start/{encoded}")),
        ] {
            let (status, code, response) = emulator.send(method, &uri, None).await;
            assert_eq!(
                (status, code.as_deref()),
                (404, Some("NotFoundException")),
                "case {case:?} / {operation}: a hostile label is answered 404 \
                 NotFoundException, got {status} {code:?}: {response}"
            );
            let message = response["message"].as_str().unwrap_or_default();
            assert!(
                message.starts_with(mc::NOT_FOUND_PREFIX),
                "case {case:?} / {operation}: the message carries the envelope, got \
                 {message:?}"
            );
            if let Some(identifier) = identifier.as_deref() {
                assert_eq!(
                    message,
                    mc::not_found_message(identifier),
                    "case {case:?} / {operation}: the message is that identifier in the \
                     envelope, got {message:?}"
                );
            }
        }
    }
}

/// One row of the not-found table: a call that addresses a resource this service does not
/// hold, and the identifier the request named it by.
struct NotFoundCase {
    case: &'static str,
    operation: &'static str,
    method: &'static str,
    uri: String,
    body: Option<Value>,
    identifier: String,
}

/// Every `NotFoundException` this service answers carries `Not found: <the identifier that
/// resolved nothing>` and nothing else — across resource kinds (flow, gateway, tagged
/// resource), across the ways an identifier can address nothing (never minted, deleted,
/// another resource type, another service, not an ARN), and whether the identifier arrived
/// as a flow label or as a tagging `ResourceArn`. A flow label that is not a flow ARN at
/// all reached no flow lookup, so the identifier it names is the generic reason rather than
/// the caller's text.
#[tokio::test]
async fn every_not_found_message_is_the_requested_identifier_in_the_services_envelope() {
    let emulator = Emulator::new();

    // A flow that existed and does not any more: the identifier is still the request's.
    let deleted = create_flow(&emulator, "deleted flow", "doomed-flow").await;
    let (status, code, body) = emulator
        .send("DELETE", &format!("/v1/flows/{deleted}"), None)
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (202, None),
        "deleting the fixture flow expected 202, got {status} {code:?}: {body}"
    );

    let unknown_flow = format!("arn:aws:mediaconnect:{REGION}:{ACCOUNT}:flow:1-0000dead:never-made");
    let unknown_gateway =
        format!("arn:aws:mediaconnect:{REGION}:{ACCOUNT}:gateway:1-0000dead:never-made");
    let other_service = format!("arn:aws:medialive:{REGION}:{ACCOUNT}:channel:1234567");

    let cases = vec![
        NotFoundCase {
            case: "a flow that was deleted",
            operation: "DescribeFlow",
            method: "GET",
            uri: format!("/v1/flows/{deleted}"),
            body: None,
            identifier: deleted.clone(),
        },
        NotFoundCase {
            case: "a flow ARN this service never minted",
            operation: "DescribeFlow",
            method: "GET",
            uri: format!("/v1/flows/{unknown_flow}"),
            body: None,
            identifier: unknown_flow.clone(),
        },
        NotFoundCase {
            case: "a flow label that is not an ARN",
            operation: "DeleteFlow",
            method: "DELETE",
            uri: "/v1/flows/not-a-valid-flow-arn".to_string(),
            body: None,
            identifier: mc::UNRESOLVED_PATH_NOT_FOUND_REASON.to_string(),
        },
        NotFoundCase {
            case: "a flow ARN this service never minted",
            operation: "AddFlowOutputs",
            method: "POST",
            uri: format!("/v1/flows/{unknown_flow}/outputs"),
            body: Some(json!({ "outputs": [
                { "name": "out", "protocol": "rtp", "destination": "203.0.113.5", "port": 5000 }
            ] })),
            identifier: unknown_flow.clone(),
        },
        NotFoundCase {
            case: "a flow ARN this service never minted",
            operation: "GrantFlowEntitlements",
            method: "POST",
            uri: format!("/v1/flows/{unknown_flow}/entitlements"),
            body: Some(json!({ "entitlements": [{ "subscribers": [SUBSCRIBER] }] })),
            identifier: unknown_flow.clone(),
        },
        NotFoundCase {
            case: "a gateway ARN this service never minted",
            operation: "DescribeGateway",
            method: "GET",
            uri: format!("/v1/gateways/{unknown_gateway}"),
            body: None,
            identifier: unknown_gateway.clone(),
        },
        NotFoundCase {
            case: "a gateway ARN this service never minted",
            operation: "DeleteGateway",
            method: "DELETE",
            uri: format!("/v1/gateways/{unknown_gateway}"),
            body: None,
            identifier: unknown_gateway.clone(),
        },
        NotFoundCase {
            case: "a resource ARN this service holds nothing for",
            operation: "ListTagsForResource",
            method: "GET",
            uri: format!("/tags/{unknown_flow}"),
            body: None,
            identifier: unknown_flow.clone(),
        },
        NotFoundCase {
            case: "another service's ARN",
            operation: "TagResource",
            method: "POST",
            uri: format!("/tags/{other_service}"),
            body: Some(json!({ "tags": { "k": "v" } })),
            identifier: other_service.clone(),
        },
        NotFoundCase {
            case: "a resource ARN this service holds nothing for",
            operation: "UntagResource",
            method: "DELETE",
            uri: format!("/tags/{unknown_flow}?tagKeys=k"),
            body: None,
            identifier: unknown_flow.clone(),
        },
    ];

    for NotFoundCase {
        case,
        operation,
        method,
        uri,
        body,
        identifier,
    } in cases
    {
        let (status, code, response) = emulator.send(method, &uri, body).await;
        assert_eq!(
            (status, code.as_deref()),
            (404, Some("NotFoundException")),
            "case {case:?} / {operation}: expected 404 NotFoundException, got {status} \
             {code:?}: {response}"
        );
        let expected = format!("{}{identifier}", mc::NOT_FOUND_PREFIX);
        assert_eq!(
            response["message"].as_str(),
            Some(expected.as_str()),
            "case {case:?} / {operation}: expected {expected:?}, got {response}"
        );
    }
}

/// One row of the value-set table: a request carrying a value outside a modelled value
/// set, and the modelled member it sat in.
struct EnumCase {
    case: &'static str,
    /// The modelled member name the refusal must name. Read off the model
    /// (`smithy_query(query="shape",shape="…")`), not off a response.
    member: &'static str,
    method: &'static str,
    uri: String,
    body: Value,
}

/// A value outside a modelled value set is refused `Validation failed:` + `Invalid
/// <Member>`, naming the member the request carried the value in — wherever in the request
/// that member sits (a top-level member, a member of a single nested structure, a member of
/// an element of a list) and whichever value set it belongs to.
#[tokio::test]
async fn a_value_outside_a_modelled_value_set_names_the_member_it_sat_in() {
    let emulator = Emulator::new();
    let flow_arn = create_flow(&emulator, "value-set fixture", "value-set-flow").await;
    let source = json!({ "name": "s", "protocol": "rtp", "whitelistCidr": "10.0.0.0/16" });
    let bogus = "not-a-real-value";

    let cases = vec![
        EnumCase {
            case: "a source protocol, in the second element of the Sources list",
            member: "Protocol",
            method: "POST",
            uri: "/v1/flows".to_string(),
            body: json!({
                "name": "vs-1",
                "sources": [
                    source.clone(),
                    { "name": "s2", "protocol": bogus, "whitelistCidr": "10.24.43.0/24" }
                ]
            }),
        },
        EnumCase {
            case: "a source protocol, in the single Source structure",
            member: "Protocol",
            method: "POST",
            uri: "/v1/flows".to_string(),
            body: json!({
                "name": "vs-2",
                "source": { "name": "s", "protocol": bogus, "whitelistCidr": "10.0.0.0/16" }
            }),
        },
        EnumCase {
            case: "FlowSize, a top-level member of the create request",
            member: "FlowSize",
            method: "POST",
            uri: "/v1/flows".to_string(),
            body: json!({ "name": "vs-3", "source": source.clone(), "flowSize": bogus }),
        },
        EnumCase {
            case: "an output's OutputStatus, in the first element of the Outputs list",
            member: "OutputStatus",
            method: "POST",
            uri: "/v1/flows".to_string(),
            body: json!({
                "name": "vs-4",
                "source": source.clone(),
                "outputs": [{
                    "name": "o", "protocol": "rtp", "destination": "203.0.113.5",
                    "port": 5000, "outputStatus": bogus
                }]
            }),
        },
        EnumCase {
            case: "an entitlement's EntitlementStatus",
            member: "EntitlementStatus",
            method: "POST",
            uri: "/v1/flows".to_string(),
            body: json!({
                "name": "vs-5",
                "source": source.clone(),
                "entitlements": [{
                    "name": "e", "subscribers": [SUBSCRIBER], "entitlementStatus": bogus
                }]
            }),
        },
        EnumCase {
            case: "a media stream's MediaStreamType",
            member: "MediaStreamType",
            method: "POST",
            uri: "/v1/flows".to_string(),
            body: json!({
                "name": "vs-6",
                "source": source.clone(),
                "mediaStreams": [{
                    "mediaStreamId": 1, "mediaStreamName": "v", "mediaStreamType": bogus
                }]
            }),
        },
        EnumCase {
            case: "the failover config's State, two structures deep",
            member: "State",
            method: "POST",
            uri: "/v1/flows".to_string(),
            body: json!({
                "name": "vs-7",
                "source": source.clone(),
                "sourceFailoverConfig": { "state": bogus }
            }),
        },
        EnumCase {
            case: "the maintenance window's MaintenanceDay",
            member: "MaintenanceDay",
            method: "POST",
            uri: "/v1/flows".to_string(),
            body: json!({
                "name": "vs-8",
                "source": source.clone(),
                "maintenance": { "maintenanceDay": bogus, "maintenanceStartHour": "13:00" }
            }),
        },
        EnumCase {
            case: "FlowSize on an update, not a create",
            member: "FlowSize",
            method: "PUT",
            uri: format!("/v1/flows/{flow_arn}"),
            body: json!({ "flowSize": bogus }),
        },
        EnumCase {
            case: "an output protocol on AddFlowOutputs, not on a create",
            member: "Protocol",
            method: "POST",
            uri: format!("/v1/flows/{flow_arn}/outputs"),
            body: json!({ "outputs": [{
                "name": "o", "protocol": bogus, "destination": "203.0.113.5", "port": 5000
            }] }),
        },
    ];

    for EnumCase {
        case,
        member,
        method,
        uri,
        body,
    } in cases
    {
        let (status, code, response) = emulator.send(method, &uri, Some(body)).await;
        assert_eq!(
            (status, code.as_deref()),
            (400, Some("BadRequestException")),
            "case {case:?}: expected 400 BadRequestException, got {status} {code:?}: {response}"
        );
        let expected = format!("{}\nInvalid {member}", mc::VALIDATION_FAILED_PREFIX);
        assert_eq!(
            response["message"].as_str(),
            Some(expected.as_str()),
            "case {case:?}: expected {expected:?}, got {response}"
        );
    }
}

struct RefusalCase {
    case: &'static str,
    /// `true` when the framework's constraint walk refuses the request before any handler
    /// runs, `false` when the handler itself does. Both must answer in one envelope.
    at_the_boundary: bool,
    method: &'static str,
    uri: String,
    body: Option<Value>,
}

/// Every `BadRequestException` this service answers ABOUT A REQUEST'S CONTENT carries
/// `Validation failed:`, a newline, then what was unacceptable — the same envelope whether
/// the framework's constraint walk refused the request (a value outside a modelled value
/// set or range, a required member absent) or a handler did (a list that must not be empty,
/// a name outside what a minted ARN admits, a bound the model does not express, a
/// pagination token the service never issued).
///
/// A well-formed request the addressed resource's STATE refuses carries the service's other
/// 400 envelope (`Bad request: `, asserted in `tests/all/lifecycle_preconditions.rs`), so
/// every case below is a content refusal.
#[tokio::test]
async fn every_refusal_message_uses_the_services_validation_envelope() {
    let emulator = Emulator::new();
    let flow_arn = create_flow(&emulator, "refusal fixture", "taken-name").await;
    let over_long_key: String = "k".repeat(mc::MAX_TAG_KEY_LENGTH + 1);

    let cases = vec![
        RefusalCase {
            case: "a source protocol outside the modelled Protocol value set",
            at_the_boundary: true,
            method: "POST",
            uri: "/v1/flows".to_string(),
            body: Some(json!({
                "name": "bad-protocol",
                "sources": [
                    { "name": "s1", "protocol": "rtp", "whitelistCidr": "10.0.0.0/16" },
                    { "name": "s2", "protocol": "not-a-real-protocol", "whitelistCidr": "10.24.43.0/24" }
                ]
            })),
        },
        RefusalCase {
            case: "the required Name member absent from a create",
            at_the_boundary: true,
            method: "POST",
            uri: "/v1/flows".to_string(),
            body: Some(json!({
                "source": { "name": "s", "protocol": "rtp", "whitelistCidr": "10.0.0.0/16" }
            })),
        },
        RefusalCase {
            case: "MaxResults below the modelled range",
            at_the_boundary: true,
            method: "GET",
            uri: "/v1/flows?maxResults=0".to_string(),
            body: None,
        },
        RefusalCase {
            case: "the required Outputs member absent from an add",
            at_the_boundary: true,
            method: "POST",
            uri: format!("/v1/flows/{flow_arn}/outputs"),
            body: Some(json!({})),
        },
        RefusalCase {
            case: "an empty Entitlements list",
            at_the_boundary: false,
            method: "POST",
            uri: format!("/v1/flows/{flow_arn}/entitlements"),
            body: Some(json!({ "entitlements": [] })),
        },
        RefusalCase {
            case: "an empty Outputs list",
            at_the_boundary: false,
            method: "POST",
            uri: format!("/v1/flows/{flow_arn}/outputs"),
            body: Some(json!({ "outputs": [] })),
        },
        RefusalCase {
            case: "an empty MediaStreams list",
            at_the_boundary: false,
            method: "POST",
            uri: format!("/v1/flows/{flow_arn}/mediaStreams"),
            body: Some(json!({ "mediaStreams": [] })),
        },
        RefusalCase {
            case: "a flow name outside the characters a minted flow ARN admits",
            at_the_boundary: false,
            method: "POST",
            uri: "/v1/flows".to_string(),
            body: Some(json!({
                "name": "bad name/here",
                "source": { "name": "s", "protocol": "rtp", "whitelistCidr": "10.0.0.0/16" }
            })),
        },
        RefusalCase {
            case: "a pagination token this service never issued",
            at_the_boundary: false,
            method: "GET",
            uri: "/v1/flows?nextToken=not-a-token".to_string(),
            body: None,
        },
        RefusalCase {
            case: "a maintenance start hour outside the documented format",
            at_the_boundary: false,
            method: "PUT",
            uri: format!("/v1/flows/{flow_arn}"),
            body: Some(json!({
                "maintenance": { "maintenanceDay": "Monday", "maintenanceStartHour": "25:77" }
            })),
        },
        RefusalCase {
            case: "a tag key longer than the documented maximum",
            at_the_boundary: false,
            method: "POST",
            uri: format!("/tags/{flow_arn}"),
            body: Some(json!({ "tags": { over_long_key.clone(): "v" } })),
        },
    ];

    for RefusalCase {
        case,
        at_the_boundary,
        method,
        uri,
        body,
    } in cases
    {
        let (status, code, response) = emulator.send(method, &uri, body).await;
        assert_eq!(
            (status, code.as_deref()),
            (400, Some("BadRequestException")),
            "case {case:?}: expected 400 BadRequestException, got {status} {code:?}: {response}"
        );
        let message = response["message"].as_str().unwrap_or_default();
        let Some(detail) = message.strip_prefix(&format!("{}\n", mc::VALIDATION_FAILED_PREFIX))
        else {
            panic!(
                "case {case:?} (refused {}): the message opens with {:?} and a newline, got \
                 {message:?}",
                if at_the_boundary {
                    "at the boundary"
                } else {
                    "by the handler"
                },
                mc::VALIDATION_FAILED_PREFIX
            );
        };
        assert!(
            !detail.trim().is_empty(),
            "case {case:?}: the envelope is followed by a detail, got {message:?}"
        );
    }
}
