// SPDX-License-Identifier: CC0-1.0

//! How this service REFUSES a request before its handler can serve it.
//!
//! Two rules are under test here, over the whole class of refusals rather than one
//! recorded request.
//!
//! 1. A value outside a constraint the MODEL declares, and a member the model
//!    requires that the request left out, are refused HTTP 400 with the error code
//!    `ValidationException` — the shape the model lists as a common error of every
//!    operation of this service — and the body carries that shape's `@required
//!    Message` member (`smithy_query(query="shape", shape="ValidationException")`:
//!    `{Message: String @required}`, `@httpError 400`), never only the JSON
//!    protocols' generic lowercase `message`. The text is AWS's constraint
//!    envelope: `<n> validation error(s) detected: ` over one clause per failing
//!    member, each naming the rejected value, the member as the wire spells it
//!    (lowerCamel) and the requirement it broke.
//!
//! 2. An identifier member has to resolve to the ARN of the resource the operation
//!    addresses, and that is decided from the request BEFORE the store is read: a
//!    value that resolves to no such ARN — a bare id that is not an id of that
//!    resource type, or an ARN whose resource segment is not
//!    `<that type>/<an id of it>` — is refused 400 `ValidationException` with
//!    `'<Member>' is not a valid resource ARN of type '<resource type>'`, not 404
//!    `ResourceNotFoundException`. A value that CAN name the resource still
//!    resolves: a bare id or an ARN of that type reads the resource back, or reports
//!    it missing.
//!
//! The tables below differ along the constraint class (range minimum and maximum,
//! enum, length, pattern, a required member left out), along the operation and HTTP
//! method, along which member carries the identifier, and along the form of the
//! identifier that cannot name the resource (a bare id of no resource type, another
//! resource type's ARN, an unknown type, an id that is not of the type's form, an
//! empty id, another Region and account).

use std::collections::HashMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::stream_workflow::{
    active_stream_group, as_label, call, call_with_headers, emulator, ready_application, text,
    ACCOUNT, REGION,
};

/// The wire error code of this service's input-validation refusal.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// An `Identifier` is modelled `length 1..128, pattern ^(^[a-zA-Z0-9-]+$)|(^arn:…$)$`,
/// so 129 identifier characters satisfy the pattern and break the length bound —
/// the one violation every operation that binds an identifier can be driven into.
fn too_long_identifier() -> String {
    "a".repeat(129)
}

/// An `Arn` is modelled `length 1..128` with the pattern
/// `^arn:aws:gameliftstreams:([^:\n]*):([0-9]{12}):([^:\n]*)$`, so this one satisfies
/// the pattern and breaks the length bound: the tagging operations' own
/// malformed-ARN refusal (`BadRequestException`) is a PATTERN failure, so a length
/// failure is the way to reach their `ValidationException`.
fn too_long_resource_arn() -> String {
    format!(
        "arn:aws:gameliftstreams:{REGION}:{ACCOUNT}:application/{}",
        "a".repeat(80)
    )
}

/// Whether `message` opens with AWS's constraint envelope.
fn opens_the_constraint_envelope(message: &str) -> bool {
    let Some((count, rest)) = message.split_once(" validation error") else {
        return false;
    };
    !count.is_empty()
        && count.chars().all(|digit| digit.is_ascii_digit())
        && (rest.starts_with(" detected: ") || rest.starts_with("s detected: "))
}

/// Assert one answer is a 400 `ValidationException` whose body carries the modelled
/// `Message` member, and return that message.
///
/// The three facts a client resolves the error from are all asserted: the status the
/// model declares, the `__type` and the `x-amzn-errortype` header restJson1 reads
/// the error SHAPE from, and the `@required Message` member — whose ABSENCE, with
/// only the framework's lowercase `message` in its place, is the defect this suite
/// pins.
fn assert_validation_exception(
    case: &str,
    status: u16,
    headers: &HashMap<String, String>,
    body: &Value,
) -> String {
    assert_eq!(
        status, 400,
        "case {case:?}: expected 400, got {status} {body}"
    );
    assert_eq!(
        body.get("__type").and_then(Value::as_str),
        Some(VALIDATION_EXCEPTION),
        "case {case:?}: expected a {VALIDATION_EXCEPTION} body, got {body}"
    );
    assert_eq!(
        headers.get("x-amzn-errortype").map(String::as_str),
        Some(VALIDATION_EXCEPTION),
        "case {case:?}: restJson1 resolves the error shape from x-amzn-errortype, got \
         {headers:?}"
    );
    assert!(
        body.get("message").is_none(),
        "case {case:?}: the modelled member is `Message`; a lowercase `message` is the \
         framework's generic envelope, got {body}"
    );
    let message = body
        .get("Message")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("case {case:?}: ValidationException models Message @required, got {body}")
        });
    assert!(
        !message.is_empty(),
        "case {case:?}: expected a non-empty Message, got {body}"
    );
    message.to_string()
}

/// One refusal whose whole message is fixed by the request and the model: the case
/// name, the request, and the exact text.
type ExactRow = (&'static str, &'static str, String, Option<Value>, String);

/// One refusal stated structurally: the case name, the request, the member the
/// clause must name (empty when the codec chooses which required member it reaches
/// first) and the requirement it must state.
type StructuralRow = (
    &'static str,
    &'static str,
    String,
    Option<Value>,
    &'static str,
    &'static str,
);

/// One identifier whose ARN form names no resource of the type the operation
/// addresses: the case name, the request, the member the message names and the
/// resource type it expects.
type ArnTypeRow = (
    String,
    &'static str,
    String,
    Option<Value>,
    &'static str,
    &'static str,
);

/// The clause a range or enum violation states is built entirely from the request
/// and the model — the rejected value, the member as the wire spells it, and the
/// bound or value set the model declares — so every one of these messages is pinned
/// exactly.
#[tokio::test]
async fn a_range_or_enum_refusal_states_the_whole_constraint_in_the_modelled_message() {
    let router = emulator();
    let rows: Vec<ExactRow> = vec![
        // ---- @range maximum, on each paginated read that binds MaxResults ----
        (
            "ListStreamSessionsByAccount above the MaxResults maximum",
            "GET",
            "/streamsessions?MaxResults=9999".to_string(),
            None,
            "1 validation error detected: Value '9999' at 'maxResults' failed to satisfy \
             constraint: Member must have value less than or equal to 100"
                .to_string(),
        ),
        (
            "ListStreamUrls one past the MaxResults maximum",
            "GET",
            "/streamurls?MaxResults=101".to_string(),
            None,
            "1 validation error detected: Value '101' at 'maxResults' failed to satisfy \
             constraint: Member must have value less than or equal to 100"
                .to_string(),
        ),
        (
            "ListApplications above the MaxResults maximum",
            "GET",
            "/applications?MaxResults=1000".to_string(),
            None,
            "1 validation error detected: Value '1000' at 'maxResults' failed to satisfy \
             constraint: Member must have value less than or equal to 100"
                .to_string(),
        ),
        // ---- @range minimum: the other published bound of the same member ----
        (
            "ListStreamGroups one below the MaxResults minimum",
            "GET",
            "/streamgroups?MaxResults=0".to_string(),
            None,
            "1 validation error detected: Value '0' at 'maxResults' failed to satisfy \
             constraint: Member must have value greater than or equal to 1"
                .to_string(),
        ),
        (
            "ListStreamSessionsByAccount below the MaxResults minimum",
            "GET",
            "/streamsessions?MaxResults=-5".to_string(),
            None,
            "1 validation error detected: Value '-5' at 'maxResults' failed to satisfy \
             constraint: Member must have value greater than or equal to 1"
                .to_string(),
        ),
        // ---- enum value sets, one row per enum this API filters a list by ----
        (
            "ListStreamUrls with a Status outside StreamUrlStatus",
            "GET",
            "/streamurls?Status=BOGUS".to_string(),
            None,
            "1 validation error detected: Value 'BOGUS' at 'status' failed to satisfy \
             constraint: Member must satisfy enum value set: [EXPIRED, ACTIVE, \
             LIMIT_REACHED, REVOKED]"
                .to_string(),
        ),
        (
            "ListStreamSessionsByAccount with a Status outside StreamSessionStatus",
            "GET",
            "/streamsessions?Status=BOGUS".to_string(),
            None,
            // The model's own value order for `StreamSessionStatus`
            // (`smithy_query(query="shape", shape="StreamSessionStatus")`).
            "1 validation error detected: Value 'BOGUS' at 'status' failed to satisfy \
             constraint: Member must satisfy enum value set: [ACTIVATING, ACTIVE, \
             CONNECTED, PENDING_CLIENT_RECONNECTION, RECONNECTING, TERMINATING, \
             TERMINATED, ERROR]"
                .to_string(),
        ),
        (
            "ListStreamSessionsByAccount with an ExportFilesStatus outside the enum",
            "GET",
            "/streamsessions?ExportFilesStatus=bogus".to_string(),
            None,
            // `smithy_query(query="shape", shape="ExportFilesStatus")`.
            "1 validation error detected: Value 'bogus' at 'exportFilesStatus' failed to \
             satisfy constraint: Member must satisfy enum value set: [SUCCEEDED, FAILED, \
             PENDING]"
                .to_string(),
        ),
        // ---- @length maximum, on an identifier label ----
        (
            "GetStreamGroup with an Identifier past its length bound",
            "GET",
            format!("/streamgroups/{}", too_long_identifier()),
            None,
            format!(
                "1 validation error detected: Value '{}' at 'identifier' failed to satisfy \
                 constraint: Member must have length less than or equal to 128",
                too_long_identifier()
            ),
        ),
        (
            "ListTagsForResource with a ResourceArn past its length bound",
            "GET",
            format!("/tags/{}", as_label(&too_long_resource_arn())),
            None,
            format!(
                "1 validation error detected: Value '{}' at 'resourceArn' failed to satisfy \
                 constraint: Member must have length less than or equal to 128",
                too_long_resource_arn()
            ),
        ),
    ];

    for (case, method, path, body, expected) in &rows {
        let (status, headers, got) = call_with_headers(&router, method, path, body.clone()).await;
        let message = assert_validation_exception(case, status, &headers, &got);
        assert_eq!(
            message, *expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
        assert!(
            !message.contains("(Internal ID:"),
            "case {case:?}: a request-validation refusal carries no internal id, got \
             {message:?}"
        );
    }
}

/// The violation classes whose clause interpolates something the test should not
/// re-spell — the model's whole regular expression, or whichever required member the
/// codec reaches first — are still stated in the modelled `Message`, inside AWS's
/// envelope, naming the member and the requirement.
#[tokio::test]
async fn a_pattern_or_absent_member_refusal_is_stated_in_the_modelled_message() {
    let router = emulator();
    // (case, method, path, body, the member the clause must name, the requirement)
    let rows: Vec<StructuralRow> = vec![
        (
            "GetStreamGroup with an Identifier outside its pattern",
            "GET",
            "/streamgroups/not%20an%20identifier".to_string(),
            None,
            "identifier",
            "Member must satisfy regular expression pattern: ",
        ),
        (
            "UpdateApplication with an Identifier outside its pattern",
            "PATCH",
            "/applications/not%20an%20identifier".to_string(),
            Some(json!({"Description": "nope"})),
            "identifier",
            "Member must satisfy regular expression pattern: ",
        ),
        (
            "CreateApplication with every required member left out",
            "POST",
            "/applications".to_string(),
            Some(json!({})),
            "",
            "Member must not be null",
        ),
        (
            "CreateStreamGroup with every required member left out",
            "POST",
            "/streamgroups".to_string(),
            Some(json!({})),
            "",
            "Member must not be null",
        ),
        (
            "AssociateApplications with its required body member left out",
            "POST",
            "/streamgroups/sg-1/associations".to_string(),
            Some(json!({})),
            "applicationIdentifiers",
            "Member must not be null",
        ),
    ];

    for (case, method, path, body, member, requirement) in &rows {
        let (status, headers, got) = call_with_headers(&router, method, path, body.clone()).await;
        let message = assert_validation_exception(case, status, &headers, &got);
        assert!(
            opens_the_constraint_envelope(&message),
            "case {case:?}: expected AWS's `<n> validation error(s) detected: ` envelope, got \
             {message:?}"
        );
        assert!(
            message.contains(requirement),
            "case {case:?}: expected the clause to state {requirement:?}, got {message:?}"
        );
        assert!(
            member.is_empty() || message.contains(&format!("at '{member}'")),
            "case {case:?}: expected the clause to name the member {member:?}, got {message:?}"
        );
    }
}

/// EVERY operation the model declares states a modelled-constraint refusal in the
/// modelled shape — implemented or still scaffolded, because a constraint is a
/// property of the input SHAPE and `ValidationException` is a common error of every
/// operation of this service. A stub answers its 501 only for a request it could
/// serve.
///
/// Each row drives exactly one operation into one modelled violation: the
/// identifier labels are driven past their length bound, the paginated reads past
/// their `MaxResults` bound, and the two operations that bind neither are driven by
/// leaving their required members out.
#[tokio::test]
async fn every_modelled_operation_states_a_constraint_refusal_in_the_modelled_shape() {
    let router = emulator();
    let long = too_long_identifier();
    let long_arn = as_label(&too_long_resource_arn());
    let locations = json!([{"LocationName": REGION, "AlwaysOnCapacity": 1}]);
    // (operation, method, path, body) — one per operation the model declares.
    let rows: Vec<(&str, &str, String, Option<Value>)> = vec![
        (
            "AddStreamGroupLocations",
            "POST",
            format!("/streamgroups/{long}/locations"),
            Some(json!({"LocationConfigurations": locations})),
        ),
        (
            "AssociateApplications",
            "POST",
            format!("/streamgroups/{long}/associations"),
            Some(json!({"ApplicationIdentifiers": ["a-1"]})),
        ),
        ("CreateApplication", "POST", "/applications".to_string(), Some(json!({}))),
        ("CreateStreamGroup", "POST", "/streamgroups".to_string(), Some(json!({}))),
        (
            "CreateStreamSessionAdminShell",
            "POST",
            format!("/streamgroups/{long}/streamsessions/ss-1/access"),
            Some(json!({})),
        ),
        (
            "CreateStreamSessionConnection",
            "POST",
            format!("/streamgroups/{long}/streamsessions/ss-1/connections"),
            Some(json!({"SignalRequest": "{\"type\":\"offer\",\"sdp\":\"v=0\"}"})),
        ),
        (
            "CreateStreamUrl",
            "POST",
            format!("/streamgroups/{long}/streamurls"),
            Some(json!({})),
        ),
        ("DeleteApplication", "DELETE", format!("/applications/{long}"), None),
        ("DeleteStreamGroup", "DELETE", format!("/streamgroups/{long}"), None),
        (
            "DisassociateApplications",
            "POST",
            format!("/streamgroups/{long}/disassociations"),
            Some(json!({"ApplicationIdentifiers": ["a-1"]})),
        ),
        (
            "ExportStreamSessionFiles",
            "PUT",
            format!("/streamgroups/{long}/streamsessions/ss-1/exportfiles"),
            Some(json!({"OutputUri": "s3://my-bucket/exports/"})),
        ),
        ("GetApplication", "GET", format!("/applications/{long}"), None),
        ("GetStreamGroup", "GET", format!("/streamgroups/{long}"), None),
        (
            "GetStreamSession",
            "GET",
            format!("/streamgroups/{long}/streamsessions/ss-1"),
            None,
        ),
        (
            "GetStreamUrl",
            "GET",
            format!("/streamgroups/{long}/streamurls/su-1"),
            None,
        ),
        (
            "ListApplicationShaderCaches",
            "GET",
            format!("/applications/{long}/shadercaches"),
            None,
        ),
        ("ListApplications", "GET", "/applications?MaxResults=0".to_string(), None),
        ("ListStreamGroups", "GET", "/streamgroups?MaxResults=0".to_string(), None),
        (
            "ListStreamSessions",
            "GET",
            format!("/streamgroups/{long}/streamsessions"),
            None,
        ),
        (
            "ListStreamSessionsByAccount",
            "GET",
            "/streamsessions?MaxResults=0".to_string(),
            None,
        ),
        ("ListStreamUrls", "GET", "/streamurls?MaxResults=0".to_string(), None),
        ("ListTagsForResource", "GET", format!("/tags/{long_arn}"), None),
        (
            "RemoveStreamGroupLocations",
            "DELETE",
            format!("/streamgroups/{long}/locations?locations={REGION}"),
            None,
        ),
        (
            "RevokeStreamUrl",
            "POST",
            format!("/streamgroups/{long}/streamurls/su-1/revoke"),
            None,
        ),
        (
            "StartStreamSession",
            "POST",
            format!("/streamgroups/{long}/streamsessions"),
            Some(json!({
                "Protocol": "WebRTC",
                "SignalRequest": "{\"type\":\"offer\",\"sdp\":\"v=0\"}",
                "ApplicationIdentifier": "a-1",
            })),
        ),
        (
            "TagResource",
            "POST",
            format!("/tags/{long_arn}"),
            Some(json!({"Tags": {"k": "v"}})),
        ),
        (
            "TerminateStreamSession",
            "DELETE",
            format!("/streamgroups/{long}/streamsessions/ss-1"),
            None,
        ),
        (
            "UntagResource",
            "DELETE",
            format!("/tags/{long_arn}?tagKeys=k"),
            None,
        ),
        (
            "UpdateApplication",
            "PATCH",
            format!("/applications/{long}"),
            Some(json!({"Description": "nope"})),
        ),
        (
            "UpdateStreamGroup",
            "PATCH",
            format!("/streamgroups/{long}"),
            Some(json!({"Description": "nope"})),
        ),
    ];
    assert_eq!(
        rows.len(),
        30,
        "the model declares 30 operations (smithy_query(query=\"digest\")); every one of \
         them must be driven here, got {} rows",
        rows.len()
    );

    for (operation, method, path, body) in &rows {
        let (status, headers, got) = call_with_headers(&router, method, path, body.clone()).await;
        let message = assert_validation_exception(operation, status, &headers, &got);
        assert!(
            opens_the_constraint_envelope(&message),
            "case {operation:?}: expected AWS's `<n> validation error(s) detected: ` envelope, \
             got {message:?}"
        );
    }
}

/// One `AssociateApplications` body that passes every modelled constraint, so the
/// only thing a row of the table below varies is the identifier under test.
fn associate_body(application: &str) -> Value {
    json!({"ApplicationIdentifiers": [application]})
}

/// A `StartStreamSession` body naming `application`.
fn start_body(application: &str) -> Value {
    json!({
        "Protocol": "WebRTC",
        "SignalRequest": "{\"type\":\"offer\",\"sdp\":\"v=0\"}",
        "ApplicationIdentifier": application,
    })
}

/// A `CreateStreamGroup` body whose `DefaultApplicationIdentifier` is the one member
/// under test.
fn create_group_body(default_application: &str) -> Value {
    json!({
        "Description": "arn-type-check",
        "StreamClass": "gen5n_high",
        "DefaultApplicationIdentifier": default_application,
        "LocationConfigurations": [{"LocationName": REGION, "AlwaysOnCapacity": 1}],
    })
}

/// An ARN of `resource_type` whose resource path is `path`, in this Region and
/// account.
fn arn(resource_type: &str, path: &str) -> String {
    format!("arn:aws:gameliftstreams:{REGION}:{ACCOUNT}:{resource_type}/{path}")
}

#[tokio::test]
async fn an_identifier_that_names_no_resource_of_the_type_is_a_validation_error() {
    let router = emulator();
    // (case, method, path, body, the member the message names, the type it names)
    let rows: Vec<ArnTypeRow> = vec![
        // ---- a BARE id that is no id of the resource type the operation addresses
        (
            "AssociateApplications with a bare Identifier that is no stream group id"
                .to_string(),
            "POST",
            "/streamgroups/not-a-group/associations".to_string(),
            Some(associate_body("a-1")),
            "Identifier",
            "streamgroup",
        ),
        (
            "GetStreamGroup with a bare Identifier that is no stream group id".to_string(),
            "GET",
            "/streamgroups/whatever".to_string(),
            None,
            "Identifier",
            "streamgroup",
        ),
        (
            "GetApplication with a bare Identifier that is no application id".to_string(),
            "GET",
            "/applications/nope".to_string(),
            None,
            "Identifier",
            "application",
        ),
        (
            "StartStreamSession with a bare ApplicationIdentifier that is no application id"
                .to_string(),
            "POST",
            "/streamgroups/sg-1/streamsessions".to_string(),
            Some(start_body("nope")),
            "ApplicationIdentifier",
            "application",
        ),
        (
            "CreateStreamGroup with a bare DefaultApplicationIdentifier that is no \
             application id"
                .to_string(),
            "POST",
            "/streamgroups".to_string(),
            Some(create_group_body("nope")),
            "DefaultApplicationIdentifier",
            "application",
        ),
        // ---- an ARN that resolves to no ARN of that type ----
        (
            "AssociateApplications with a streamgroup ARN whose id is not a stream group id"
                .to_string(),
            "POST",
            format!(
                "/streamgroups/{}/associations",
                as_label(&arn("streamgroup", "not-a-group"))
            ),
            Some(associate_body("a-1")),
            "Identifier",
            "streamgroup",
        ),
        (
            "AssociateApplications with an application ARN as the stream group".to_string(),
            "POST",
            format!(
                "/streamgroups/{}/associations",
                as_label(&arn("application", "a-1"))
            ),
            Some(associate_body("a-1")),
            "Identifier",
            "streamgroup",
        ),
        (
            "AssociateApplications with an ApplicationIdentifiers entry that is no \
             application ARN"
                .to_string(),
            "POST",
            format!("/streamgroups/{}/associations", as_label(&arn("streamgroup", "sg-1"))),
            Some(associate_body(&arn("application", "nope"))),
            "ApplicationIdentifiers",
            "application",
        ),
        (
            "GetStreamGroup with an ARN of an unknown resource type".to_string(),
            "GET",
            format!("/streamgroups/{}", as_label(&arn("gamesession", "sg-1"))),
            None,
            "Identifier",
            "streamgroup",
        ),
        (
            "DeleteStreamGroup with a streamgroup ARN whose id is empty".to_string(),
            "DELETE",
            format!("/streamgroups/{}", as_label(&arn("streamgroup", "sg-"))),
            None,
            "Identifier",
            "streamgroup",
        ),
        (
            "UpdateStreamGroup with a streamgroup ARN in another Region and account"
                .to_string(),
            "PATCH",
            format!(
                "/streamgroups/{}",
                as_label("arn:aws:gameliftstreams:eu-west-1:111122223333:streamgroup/nope")
            ),
            Some(json!({"Description": "unreachable"})),
            "Identifier",
            "streamgroup",
        ),
        (
            "AddStreamGroupLocations with an application ARN as the stream group".to_string(),
            "POST",
            format!(
                "/streamgroups/{}/locations",
                as_label(&arn("application", "a-1"))
            ),
            Some(json!({"LocationConfigurations": [{"LocationName": REGION, "AlwaysOnCapacity": 1}]})),
            "Identifier",
            "streamgroup",
        ),
        (
            "StartStreamSession with a streamgroup ARN whose id is not a stream group id"
                .to_string(),
            "POST",
            format!(
                "/streamgroups/{}/streamsessions",
                as_label(&arn("streamgroup", "whatever"))
            ),
            Some(start_body("a-1")),
            "Identifier",
            "streamgroup",
        ),
        (
            "StartStreamSession with an ApplicationIdentifier that is no application ARN"
                .to_string(),
            "POST",
            format!(
                "/streamgroups/{}/streamsessions",
                as_label(&arn("streamgroup", "sg-1"))
            ),
            Some(start_body(&arn("streamgroup", "sg-1"))),
            "ApplicationIdentifier",
            "application",
        ),
        (
            "GetApplication with a streamgroup ARN as the application".to_string(),
            "GET",
            format!("/applications/{}", as_label(&arn("streamgroup", "sg-1"))),
            None,
            "Identifier",
            "application",
        ),
        (
            "DeleteApplication with an application ARN whose id is not an application id"
                .to_string(),
            "DELETE",
            format!("/applications/{}", as_label(&arn("application", "nope"))),
            None,
            "Identifier",
            "application",
        ),
        (
            "CreateStreamGroup with a DefaultApplicationIdentifier that is no application ARN"
                .to_string(),
            "POST",
            "/streamgroups".to_string(),
            Some(create_group_body(&arn("application", "nope"))),
            "DefaultApplicationIdentifier",
            "application",
        ),
        (
            "UpdateStreamGroup with a DefaultApplicationIdentifier that is no application ARN"
                .to_string(),
            "PATCH",
            format!("/streamgroups/{}", as_label(&arn("streamgroup", "sg-1"))),
            Some(json!({"DefaultApplicationIdentifier": arn("streamgroup", "sg-1")})),
            "DefaultApplicationIdentifier",
            "application",
        ),
        (
            "GetStreamSession with an application ARN as the stream session".to_string(),
            "GET",
            format!(
                "/streamgroups/sg-1/streamsessions/{}",
                as_label(&arn("application", "a-1"))
            ),
            None,
            "StreamSessionIdentifier",
            "streamsession",
        ),
        (
            "TerminateStreamSession with a streamsession ARN whose path is empty".to_string(),
            "DELETE",
            format!(
                "/streamgroups/sg-1/streamsessions/{}",
                as_label(&arn("streamsession", ""))
            ),
            None,
            "StreamSessionIdentifier",
            "streamsession",
        ),
    ];

    for (case, method, path, body, member, resource_type) in &rows {
        let (status, headers, got) = call_with_headers(&router, method, path, body.clone()).await;
        let message = assert_validation_exception(case, status, &headers, &got);
        assert_eq!(
            message,
            format!("'{member}' is not a valid resource ARN of type '{resource_type}'"),
            "case {case:?}: expected the member and the expected resource type, got {message:?}"
        );
        assert!(
            !message.contains("(Internal ID:"),
            "case {case:?}: a request-validation refusal carries no internal id, got {message:?}"
        );
    }
}

/// The other side of the same rule: an identifier that CAN name a resource of the
/// type the operation addresses is resolved, so it answers 404 when the store holds
/// no such resource and 200 when it does. The rows differ in the identifier's form —
/// a bare id of that type, an ARN in this Region and account, an ARN elsewhere, and
/// the composite stream session ARN — so the check cannot be passing by refusing
/// ARNs, or ids, in general.
#[tokio::test]
async fn an_identifier_that_can_name_the_resource_is_still_resolved() {
    let router = emulator();
    let (app_id, app_arn) = ready_application(&router, "arn-type-app").await;
    let (group_id, group_arn) =
        active_stream_group(&router, "arn-type-group", &app_arn, &[REGION]).await;

    // (case, method, path) — every one of these resolves, and finds nothing.
    let missing: Vec<(&str, &str, String)> = vec![
        ("GetStreamGroup by bare id", "GET", "/streamgroups/sg-gone".to_string()),
        (
            "GetStreamGroup by a conforming ARN",
            "GET",
            format!("/streamgroups/{}", as_label(&arn("streamgroup", "sg-gone"))),
        ),
        (
            "GetStreamGroup by a conforming ARN in another Region and account",
            "GET",
            format!(
                "/streamgroups/{}",
                as_label("arn:aws:gameliftstreams:eu-west-1:111122223333:streamgroup/sg-gone")
            ),
        ),
        ("GetApplication by bare id", "GET", "/applications/a-gone".to_string()),
        (
            "GetApplication by a conforming ARN",
            "GET",
            format!("/applications/{}", as_label(&arn("application", "a-gone"))),
        ),
        (
            "GetStreamSession by a conforming composite ARN",
            "GET",
            format!(
                "/streamgroups/{group_id}/streamsessions/{}",
                as_label(&arn("streamsession", &format!("{group_id}/ss-gone")))
            ),
        ),
        (
            "AssociateApplications with a conforming application ARN that is not there",
            "POST",
            format!("/streamgroups/{group_id}/associations"),
        ),
    ];
    for (case, method, path) in &missing {
        let body = path
            .ends_with("/associations")
            .then(|| associate_body(&arn("application", "a-gone")));
        let (status, got) = call(&router, method, path, body).await;
        assert_eq!(
            status, 404,
            "case {case:?}: an identifier that can name the resource is resolved, so the \
             answer is a 404, got {status} {got}"
        );
    }

    // And a conforming ARN of a resource that IS there reads it back.
    let (status, got) = call(
        &router,
        "GET",
        &format!("/streamgroups/{}", as_label(&group_arn)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"GetStreamGroup by the ARN of a live group\": expected 200, got {status} {got}"
    );
    assert_eq!(
        text(&got, "/Arn", "a live stream group by ARN"),
        group_arn,
        "case \"GetStreamGroup by the ARN of a live group\": expected the stored group, got \
         {got}"
    );
    let (status, got) = call(
        &router,
        "GET",
        &format!("/applications/{}", as_label(&app_arn)),
        None,
    )
    .await;
    assert_eq!(
        (status, text(&got, "/Id", "a live application by ARN")),
        (200, app_id),
        "case \"GetApplication by the ARN of a live application\": expected the stored \
         application, got {got}"
    );
}

/// The malformed-`ResourceArn` refusal of the tagging operations is NOT reworded by
/// the modelled-constraint path: that error code (`BadRequestException`) is in no
/// model of this service, so it has no modelled `Message` member to carry and keeps
/// the framework's envelope. Pinned here because the two refusals travel the same
/// seam, and only one of them is the modelled shape.
#[tokio::test]
async fn a_malformed_resource_arn_keeps_its_own_error_code() {
    let router: MultiServiceRouter = emulator();
    for (case, arn_label) in [
        ("not an ARN at all", "not-an-arn"),
        ("another service", "arn:aws:s3:::my-bucket"),
    ] {
        let (status, headers, got) =
            call_with_headers(&router, "GET", &format!("/tags/{arn_label}"), None).await;
        assert_eq!(
            (status, got.get("__type").and_then(Value::as_str)),
            (400, Some("BadRequestException")),
            "case {case:?}: expected 400 BadRequestException, got {status} {got}"
        );
        assert_eq!(
            headers.get("x-amzn-errortype").map(String::as_str),
            Some("BadRequestException"),
            "case {case:?}: the error shape travels in x-amzn-errortype too, got {headers:?}"
        );
    }
}
