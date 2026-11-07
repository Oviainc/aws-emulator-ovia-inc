// SPDX-License-Identifier: CC0-1.0

//! The Directory Service Data REQUEST LAYER: everything the service rejects
//! before an operation handler runs, end to end over HTTP.
//!
//! The rule under test is the layering of that request layer and the envelope
//! each of its layers answers in:
//!
//! 1. the URI resolves an operation, or the request is answered as naming none —
//!    `200` with the unknown-operation envelope on a method this service serves
//!    operations on, `404` on any other method;
//! 2. the body is read into the modeled input shape, or the request is answered
//!    `400 SerializationException`;
//! 3. the members bound to the request are present, or the request is answered
//!    the operation's own `400 ValidationException` naming the absent one.
//!
//! No layer answers `501`: a 501 states that an operation exists but is not
//! implemented, which is neither true of this service nor the remediation a
//! caller needs. Every case drives the router, so it exercises the same path a
//! client takes.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_directoryservicedata_directoryservicedata::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A `DirectoryId` satisfying `^d-[0-9a-f]{10}$`.
const DIRECTORY: &str = "d-1234567890";

/// Every operation's `@http` URI, from
/// `smithy_query(query="digest")` — all 17 are `POST` on a literal path, and all
/// 17 mark `DirectoryId` `@required` + `@httpQuery("DirectoryId")`, which is why
/// one table covers the whole service.
const OPERATION_URIS: [&str; 17] = [
    "/GroupMemberships/AddGroupMember",
    "/GroupMemberships/ListGroupMembers",
    "/GroupMemberships/ListGroupsForMember",
    "/GroupMemberships/RemoveGroupMember",
    "/Groups/CreateGroup",
    "/Groups/DeleteGroup",
    "/Groups/DescribeGroup",
    "/Groups/ListGroups",
    "/Groups/SearchGroups",
    "/Groups/UpdateGroup",
    "/Users/CreateUser",
    "/Users/DeleteUser",
    "/Users/DescribeUser",
    "/Users/DisableUser",
    "/Users/ListUsers",
    "/Users/SearchUsers",
    "/Users/UpdateUser",
];

/// The `x-amzn-errortype` a MODELED error of this service is named by: the error
/// shape's name, as the protocol's modelled-error serializer writes it for every
/// modelled error of this service — so a refusal this service owns is named one
/// way wherever it is raised.
const MODELED_VALIDATION_ERRORTYPE: &str = "ValidationException";

/// The `x-amzn-errortype` the request layer's own errors are named by: their
/// shape is not this service's but `com.amazon.coral.service`'s, so the name
/// carries the URI of the namespace that declares it — the namespace the
/// unknown-operation envelope names in its `__type`.
const SERIALIZATION_ERRORTYPE: &str =
    "SerializationException:http://internal.amazon.com/coral/com.amazon.coral.service/";

/// A SigV4 credential scope naming this service: `ds-data` is its `sigv4Name`,
/// and the route key is what claims the request for this service.
const SIGNED_FOR_DS_DATA: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/ds-data/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// One emulator: a router carrying only this service, over one shared engine, with the
/// account holding the directory these cases address — every operation of this data plane
/// is gated on that, so a request that is meant to reach its handler needs it.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    super::validation_envelope::hold_directories(&engine, &[DIRECTORY]);
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the status, the `x-amzn-errortype` header and the
/// body, parsed when it is JSON.
struct Answer {
    status: u16,
    errortype: String,
    raw: String,
    body: Value,
}

impl Answer {
    /// A string member of the answer, or the empty string when it carries none.
    fn text(&self, member: &str) -> String {
        self.body[member].as_str().unwrap_or_default().to_string()
    }

    /// The reason the answer gives, wherever it carries it: this service's error
    /// shapes name it `Message`, while the framework's pre-dispatch envelope
    /// spells the same member `message` (the difference is the related open
    /// issue's, not this one's, so the assertions below read either).
    fn reason(&self) -> String {
        match self.body.get("Message").or_else(|| self.body.get("message")) {
            Some(Value::String(reason)) => reason.clone(),
            _ => String::new(),
        }
    }
}

/// Send one raw request — method, URI and body bytes exactly as given, so a
/// malformed body and an unrouted URI are reachable.
async fn send(router: &MultiServiceRouter, method: &str, uri: &str, body: &str) -> Answer {
    let request = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "ds-data.us-east-1.amazonaws.com")
        .header("authorization", SIGNED_FOR_DS_DATA)
        .header("content-type", "application/json")
        .body(Bytes::from(body.to_string()))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let errortype = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let bytes = response.into_body();
    let raw = String::from_utf8_lossy(&bytes).to_string();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    Answer {
        status,
        errortype,
        raw,
        body,
    }
}

/// `POST <uri>` with a JSON body, the shape every call of this service takes.
async fn post(router: &MultiServiceRouter, uri: &str, body: &Value) -> Answer {
    send(router, "POST", uri, &body.to_string()).await
}

/// A request on a method this service serves operations on (`POST`, on all 17)
/// that resolves no operation is answered 200 with the unknown-operation
/// envelope: it reached the layer that names an operation, so what it could not
/// name is reported inside the envelope and not as a transport failure.
#[tokio::test]
async fn a_request_resolving_no_operation_answers_the_unknown_operation_envelope() {
    let router = emulator();
    let expected = json!({
        "Output": {"__type": "com.amazon.coral.service#UnknownOperationException"},
        "Version": "1.0",
    });

    let cases = [
        ("the bare endpoint", "/", ""),
        ("the endpoint with a body", "/", r#"{"DirectoryId":"d-1234567890"}"#),
        ("the endpoint with a query", "/?DirectoryId=d-1234567890", ""),
        ("the endpoint with a malformed body", "/", "{not json"),
        ("an unknown operation on a served resource", "/Users/NotAnOperation", ""),
        ("a served operation on an unknown resource", "/NotAResource/ListUsers", ""),
        ("one segment too many", "/Users/ListUsers/Extra", ""),
        ("one segment too few", "/Users", ""),
        ("a case-shifted path", "/users/listusers", ""),
        (
            "an unknown operation carrying every parameter",
            "/Users/NotAnOperation?DirectoryId=d-1234567890",
            r#"{"MaxResults":1}"#,
        ),
    ];
    for (case, uri, body) in cases {
        let answer = send(&router, "POST", uri, body).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200, got {} with {:?}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.body, expected,
            "case {case:?}: expected the unknown-operation envelope, got {:?}",
            answer.raw
        );
        assert_eq!(
            answer.errortype, "",
            "case {case:?}: a 200 names no error type, got {:?}",
            answer.errortype
        );
    }
}

/// A request on a method this service serves no operation on is 404 with the
/// exception as the body's key — never 501, which would tell the caller the
/// operation exists but is unimplemented.
#[tokio::test]
async fn a_method_the_service_serves_nothing_on_is_not_found() {
    let router = emulator();
    let cases = [
        ("a GET of a served route", "GET", "/Users/ListUsers"),
        ("a DELETE of a served route", "DELETE", "/Groups/DeleteGroup"),
        ("a PUT of a served route", "PUT", "/Groups/UpdateGroup"),
        ("a GET of the endpoint", "GET", "/"),
        ("a PATCH of an unknown path", "PATCH", "/NotAResource/ListUsers"),
        (
            "a GET of a served route carrying every parameter",
            "GET",
            "/Users/ListUsers?DirectoryId=d-1234567890",
        ),
    ];
    for (case, method, uri) in cases {
        let answer = send(&router, method, uri, "").await;
        assert_eq!(
            answer.status, 404,
            "case {case:?} ({method} {uri}): expected HTTP 404, got {} with {:?}",
            answer.status, answer.raw
        );
        assert!(
            answer.body.get("UnknownOperationException").is_some(),
            "case {case:?} ({method} {uri}): expected an UnknownOperationException body key, got {:?}",
            answer.raw
        );
    }
}

/// A request that omits the member every operation binds to the query string is
/// the operation's own validation refusal, naming that member — not a routing
/// failure. Asserted on every operation the service serves, because the binding
/// is on all of them.
#[tokio::test]
async fn an_absent_request_bound_member_is_a_validation_refusal() {
    let router = emulator();
    let expected_message = "1 validation error detected: Value null at 'directoryId' \
         failed to satisfy constraint: Member must not be null";

    for uri in OPERATION_URIS {
        let answer = post(&router, uri, &json!({"SAMAccountName": "stored"})).await;
        assert_eq!(
            answer.status, 400,
            "case {uri:?}: expected HTTP 400, got {} with {:?}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.errortype, MODELED_VALIDATION_ERRORTYPE,
            "case {uri:?}: expected {MODELED_VALIDATION_ERRORTYPE:?}, got {:?}",
            answer.errortype
        );
        assert_eq!(
            answer.text("Message"),
            expected_message,
            "case {uri:?}: expected the absent-member message, got {:?}",
            answer.raw
        );
    }

    // The member being PRESENT is what the refusal is about, not its value: an
    // empty or malformed one reaches the operation and is refused on its own
    // constraint instead.
    for (case, query) in [
        ("empty", "?DirectoryId="),
        ("malformed", "?DirectoryId=not-an-id"),
        ("an unrelated parameter beside it", "?Other=1&DirectoryId=not-an-id"),
    ] {
        let answer = post(
            &router,
            &format!("/Users/ListUsers{query}"),
            &json!({}),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with {:?}",
            answer.status, answer.raw
        );
        assert!(
            !answer.raw.contains("must not be null"),
            "case {case:?}: a present member must not be reported absent, got {:?}",
            answer.raw
        );
    }
}

/// A refusal states the constraint the service CHECKS: a `@pattern` is a
/// full-string match, so the clause names the modeled expression without the
/// anchors that pin it to the whole string. One row per constrained member
/// family, with each expression taken from
/// `smithy_query(query="operation")`'s `pattern` for that member.
#[tokio::test]
async fn a_pattern_refusal_states_the_expression_it_checks() {
    let router = emulator();
    // (case, uri, body, the modeled expression without its anchors)
    let cases = [
        (
            "directoryId",
            "/Users/ListUsers?DirectoryId=NOT-AN-ID".to_string(),
            json!({}),
            r"d-[0-9a-f]{10}",
        ),
        (
            "sAMAccountName as a user name",
            format!("/Users/CreateUser?DirectoryId={DIRECTORY}"),
            json!({"SAMAccountName": "bad name!"}),
            r"[\w\-.]+",
        ),
        (
            "sAMAccountName as a group name",
            format!("/Groups/DescribeGroup?DirectoryId={DIRECTORY}"),
            json!({"SAMAccountName": "sim2real;group"}),
            r#"[^:;|=+"*?<>/\\,\[\]@]+"#,
        ),
        (
            "realm",
            format!("/Groups/ListGroups?DirectoryId={DIRECTORY}"),
            json!({"Realm": "not a realm!"}),
            r"([a-zA-Z0-9]+[\\.-])+([a-zA-Z0-9])+[.]?",
        ),
    ];
    for (case, uri, body, expression) in cases {
        let answer = post(&router, &uri, &body).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with {:?}",
            answer.status, answer.raw
        );
        let expected = format!("Member must satisfy regular expression pattern: {expression}");
        assert!(
            answer.reason().ends_with(&expected),
            "case {case:?}: expected a clause ending {expected:?}, got {:?}",
            answer.raw
        );
        assert!(
            !answer.reason().contains(&format!("^{expression}$")),
            "case {case:?}: the clause must not state the anchors, got {:?}",
            answer.raw
        );
    }
}

/// A body that is not JSON cannot be read into any shape, so no member can be
/// named: the service answers `400 SerializationException` with an empty body.
#[tokio::test]
async fn an_unreadable_body_is_a_serialization_refusal() {
    let router = emulator();
    let cases = [
        ("truncated object", "{"),
        ("not JSON at all", "this is not json"),
        ("a key with no value", r#"{"MaxResults":}"#),
        ("unterminated array", "[1,2"),
        ("trailing garbage", r#"{"MaxResults":1}junk"#),
        ("an unquoted key", "{MaxResults: 1}"),
        ("binary noise", "\u{1}\u{2}\u{3}"),
    ];
    for (case, body) in cases {
        for uri in [
            "/Users/ListUsers?DirectoryId=d-1234567890",
            "/Groups/ListGroups?DirectoryId=d-1234567890",
        ] {
            let answer = send(&router, "POST", uri, body).await;
            assert_eq!(
                answer.status, 400,
                "case {case:?} on {uri:?}: expected HTTP 400, got {} with {:?}",
                answer.status, answer.raw
            );
            assert_eq!(
                answer.errortype, SERIALIZATION_ERRORTYPE,
                "case {case:?} on {uri:?}: expected {SERIALIZATION_ERRORTYPE:?}, got {:?}",
                answer.errortype
            );
            assert_eq!(
                answer.raw, "{}",
                "case {case:?} on {uri:?}: expected an empty body, got {:?}",
                answer.raw
            );
        }
    }
}

/// A value whose JSON type cannot be coerced into the modelled target type of
/// the member it is bound to is `400 SerializationException` naming the CLASS of
/// value sent and the type it could not become — a deserialization failure, not
/// a constraint violation. The refusal names the class (`STRING_VALUE`, …) and
/// never the value itself, so a caller cannot amplify a long value into a long
/// response. One row per class the rule generalises over: the target type and
/// the JSON class sent, at both the levels the request layer reads — the body
/// document itself and the input's own members. (A value nested inside a list or
/// a map is read by the codec: a member schema carries its target's kind but not
/// its target's schema, so a collection's element shape is not reachable.)
#[tokio::test]
async fn a_value_of_the_wrong_json_type_is_a_serialization_refusal() {
    let router = emulator();
    // (case, uri, body, the message the refusal states)
    let cases = [
        (
            "a string where an integer is modeled",
            "/Users/ListUsers",
            json!({"MaxResults": "STRING_VALUE"}),
            "STRING_VALUE can not be converted to an Integer",
        ),
        (
            "a numeric string where an integer is modeled",
            "/Users/ListUsers",
            json!({"MaxResults": "10"}),
            "STRING_VALUE can not be converted to an Integer",
        ),
        (
            "a boolean where an integer is modeled",
            "/Users/ListUsers",
            json!({"MaxResults": true}),
            "BOOLEAN_VALUE can not be converted to an Integer",
        ),
        (
            "a fraction where an integer is modeled",
            "/Users/ListUsers",
            json!({"MaxResults": 1.5}),
            "NUMBER_VALUE can not be converted to an Integer",
        ),
        (
            "a number past the integer width",
            "/Users/ListUsers",
            json!({"MaxResults": 2_147_483_648_i64}),
            "NUMBER_VALUE can not be converted to an Integer",
        ),
        (
            "a list where an integer is modeled",
            "/Groups/ListGroups",
            json!({"MaxResults": []}),
            "ARRAY_VALUE can not be converted to an Integer",
        ),
        (
            "an object where an integer is modeled",
            "/Groups/ListGroups",
            json!({"MaxResults": {"n": 1}}),
            "OBJECT_VALUE can not be converted to an Integer",
        ),
        (
            "a number where a string is modeled",
            "/Users/ListUsers",
            json!({"Realm": 42}),
            "NUMBER_VALUE can not be converted to a String",
        ),
        (
            "a number where an enum's string is modeled",
            "/Groups/CreateGroup",
            json!({"SAMAccountName": "g", "GroupType": 7}),
            "NUMBER_VALUE can not be converted to a String",
        ),
        (
            "a string where a list is modeled",
            "/Users/SearchUsers",
            json!({"SearchString": "s", "SearchAttributes": "GivenName"}),
            "STRING_VALUE can not be converted to a List",
        ),
        (
            "a list where a map is modeled",
            "/Users/CreateUser",
            json!({"SAMAccountName": "u", "OtherAttributes": []}),
            "ARRAY_VALUE can not be converted to a Map",
        ),
        (
            "a body that is not the modeled structure",
            "/Users/ListUsers",
            json!([1, 2]),
            "ARRAY_VALUE can not be converted to a Structure",
        ),
        (
            "a body that is a bare string",
            "/Users/DescribeUser",
            json!("stored"),
            "STRING_VALUE can not be converted to a Structure",
        ),
    ];
    for (case, path, body, expected_message) in cases {
        let uri = format!("{path}?DirectoryId={DIRECTORY}");
        let answer = post(&router, &uri, &body).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with {:?}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.errortype, SERIALIZATION_ERRORTYPE,
            "case {case:?}: expected {SERIALIZATION_ERRORTYPE:?}, got {:?}",
            answer.errortype
        );
        assert_eq!(
            answer.text("Message"),
            expected_message,
            "case {case:?}: expected the coercion message, got {:?}",
            answer.raw
        );
    }
}

/// A value of the RIGHT JSON type is not a serialization refusal, whatever else
/// is wrong with it: the layer below (constraint validation) owns it, and a
/// client branches on the difference. The non-vacuity guard for the case above.
#[tokio::test]
async fn a_value_of_the_right_json_type_is_not_a_serialization_refusal() {
    let router = emulator();
    let cases = [
        (
            "a whole number inside the integer width",
            "/Users/ListUsers",
            json!({"MaxResults": 10}),
        ),
        (
            "an integer outside its @range",
            "/Users/ListUsers",
            json!({"MaxResults": 251}),
        ),
        (
            "a string outside its @pattern",
            "/Groups/ListGroups",
            json!({"Realm": "not a realm!"}),
        ),
        (
            "a string outside its @length",
            "/Users/CreateUser",
            json!({"SAMAccountName": ""}),
        ),
        (
            "a value outside an enum's set",
            "/Groups/CreateGroup",
            json!({"SAMAccountName": "g", "GroupType": "NotAGroupType"}),
        ),
        (
            "a list element outside its @pattern",
            "/Users/SearchUsers",
            json!({"SearchString": "s", "SearchAttributes": ["1BadAttribute"]}),
        ),
        (
            "a member the shape does not model",
            "/Users/ListUsers",
            json!({"NotAModeledMember": "ignored"}),
        ),
        (
            "an explicit null",
            "/Users/ListUsers",
            json!({"MaxResults": null}),
        ),
        ("no body members at all", "/Users/ListUsers", json!({})),
    ];
    for (case, path, body) in cases {
        let uri = format!("{path}?DirectoryId={DIRECTORY}");
        let answer = post(&router, &uri, &body).await;
        assert!(
            !answer.errortype.starts_with("SerializationException"),
            "case {case:?}: a readable value is not a serialization failure, got {} {:?} {:?}",
            answer.status,
            answer.errortype,
            answer.raw
        );
        assert_ne!(
            answer.status, 501,
            "case {case:?}: this service never answers 501, got {:?}",
            answer.raw
        );
    }
}

/// The layers answer in order: the URI first, then the body, then the members
/// bound to the request. A request that is wrong at two layers is answered by
/// the outer one, which is the one the caller has to fix first.
#[tokio::test]
async fn the_request_layers_answer_in_order() {
    let router = emulator();

    // An unknown URI with a malformed body: no operation is named, so nothing
    // reads the body.
    let answer = send(&router, "POST", "/Users/NotAnOperation", "{not json").await;
    assert_eq!(
        answer.status, 200,
        "case \"unknown uri with a malformed body\": expected HTTP 200, got {} with {:?}",
        answer.status, answer.raw
    );

    // A malformed body on a known URI that also omits the query member: the body
    // is read before anything is validated, so it is the serialization refusal.
    let answer = send(&router, "POST", "/Users/ListUsers", "{not json").await;
    assert_eq!(
        answer.errortype, SERIALIZATION_ERRORTYPE,
        "case \"malformed body and absent member\": expected {SERIALIZATION_ERRORTYPE:?}, got {:?} ({:?})",
        answer.errortype, answer.raw
    );

    // A readable body that omits the query member: the validation refusal.
    let answer = send(&router, "POST", "/Users/ListUsers", r#"{"MaxResults":1}"#).await;
    assert_eq!(
        answer.errortype, MODELED_VALIDATION_ERRORTYPE,
        "case \"absent member with a readable body\": expected {MODELED_VALIDATION_ERRORTYPE:?}, got {:?} ({:?})",
        answer.errortype, answer.raw
    );
}

/// The request layer refuses only what it must: a well-formed request still
/// reaches its handler and round-trips through the stored record.
#[tokio::test]
async fn a_well_formed_request_still_reaches_its_operation() {
    let router = emulator();

    let created = post(
        &router,
        &format!("/Users/CreateUser?DirectoryId={DIRECTORY}"),
        &json!({"SAMAccountName": "stored", "GivenName": "Ada"}),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case \"CreateUser\": expected HTTP 200, got {} with {:?}",
        created.status, created.raw
    );

    let described = post(
        &router,
        &format!("/Users/DescribeUser?DirectoryId={DIRECTORY}"),
        &json!({"SAMAccountName": "stored"}),
    )
    .await;
    assert_eq!(
        described.status, 200,
        "case \"DescribeUser\": expected HTTP 200, got {} with {:?}",
        described.status, described.raw
    );
    assert_eq!(
        described.text("SAMAccountName"),
        "stored",
        "case \"DescribeUser\": expected the stored user, got {:?}",
        described.raw
    );

    let listed = post(
        &router,
        &format!("/Users/ListUsers?DirectoryId={DIRECTORY}&MaxResults=10"),
        &json!({"MaxResults": 10}),
    )
    .await;
    assert_eq!(
        listed.status, 200,
        "case \"ListUsers\": expected HTTP 200, got {} with {:?}",
        listed.status, listed.raw
    );
    assert_eq!(
        listed.body["Users"].as_array().map(Vec::len),
        Some(1),
        "case \"ListUsers\": expected the one stored user, got {:?}",
        listed.raw
    );
}
