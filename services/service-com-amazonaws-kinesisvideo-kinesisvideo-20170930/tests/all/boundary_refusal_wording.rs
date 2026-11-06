// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage for how this service WORDS a request the boundary
//! validator refused before any handler ran.
//!
//! RULE: a constraint refusal is `InvalidArgumentException` (400) whose text reaches the
//! caller in the `Message` member every error shape of this model declares, and is the
//! validation envelope over one clause per refused member — the member's lowerCamel
//! path and the requirement its value had to satisfy — and the clause states NO
//! rejected value, whatever constraint class it broke. The requirement is a property of
//! the model, so the same request always produces the same sentence and no
//! caller-supplied bytes reach it; a `@pattern` requirement quotes the expression
//! without the whole-value anchors the model wraps it in. A member the request did not
//! carry at all is still stated as the `null` it was, which is a fact about the request
//! and not a value out of it.
//!
//! Every case drives the real request walk (route -> deserialize -> validate -> handle
//! -> serialize) through [`MultiServiceRouter::dispatch`], so a response here is the
//! same bytes a client on the socket would see.
//!
//! The harness mirrors the one in the sibling suites of this crate (each test target
//! module owns its own, since none of them is importable from another).

use std::sync::Arc;

use service_com_amazonaws_kinesisvideo_kinesisvideo_20170930 as kvs;
use smithy_server_runtime::MultiServiceRouter;

/// A router carrying only this service, over its own engine, with request
/// authentication off so a case can send an unsigned-but-scoped request.
fn harness() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(kvs::EmulatedService::new(engine));
    router
}

/// The emulator endpoint every case addresses.
const ENDPOINT_HOST: &str = "kinesisvideo.us-east-1.amazonaws.com";

/// The error code this service refuses a bad input value with.
const INVALID_ARGUMENT: &str = "InvalidArgumentException";

/// POST one restJson1 call to `path`, answering its status, its parsed body and its
/// `x-amzn-errortype` header.
async fn call(
    router: &MultiServiceRouter,
    path: &str,
    body: serde_json::Value,
) -> (u16, serde_json::Value, String) {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{ENDPOINT_HOST}{path}"))
        .header("host", ENDPOINT_HOST)
        .header("content-type", "application/json")
        // The SigV4 credential scope is what routes the request to this service and
        // names its region; the signature itself is not checked here.
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/kinesisvideo/aws4_request, \
             SignedHeaders=host, Signature=0",
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
    };
    (status, parsed, error_type)
}

/// The refusal's text, read from the ONE member this service's error shapes declare it in
/// — not from the synthetic lowercase member a framework-rendered body would carry, so a
/// refusal that reached the caller under the wrong member reads as no text at all here.
fn message(body: &serde_json::Value) -> String {
    body.get("Message")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The wire error code of a failure body (`__type`), with any Smithy namespace stripped.
fn error_code(body: &serde_json::Value) -> String {
    body.get("__type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .rsplit('#')
        .next()
        .unwrap_or("")
        .to_string()
}

/// One refused request: what it is, where it goes, what it sends, the clause it must be
/// stated with, and the bytes of it that must NOT appear in that clause.
struct Row {
    case: &'static str,
    path: &'static str,
    request: serde_json::Value,
    clause: String,
    rejected: String,
}

/// RULE: every constraint class is refused with `InvalidArgumentException` (400) and the
/// one envelope above — the member path and its requirement, with no rejected value.
///
/// The rows differ along what the rule generalises over: the constraint class (an
/// enumerated type's value set, `@range`, `@length` on a string, `@length` on a list,
/// `@length` on a map, `@pattern`), the operation carrying it, and how deep in the input
/// the member sits (a top-level member, and one nested inside a structure member). Every
/// bound and value set quoted below is the model's, from
/// `smithy_query(query="shape", shape="ComparisonOperator,ResourceARN,StreamName,ListStreamsInput,TagKeyList,ResourceTags,APIName")`.
#[tokio::test]
async fn a_constraint_refusal_states_the_requirement_and_no_rejected_value() {
    let router = harness();

    // Past `StreamName`'s `@length` maximum of 256, while still inside its `@pattern`,
    // so the length is the one constraint the request breaks.
    let over_long_name = "n".repeat(300);
    // Past `ResourceTags`' `@length` maximum of 50 entries.
    let over_long_tags: serde_json::Map<String, serde_json::Value> = (0..51)
        .map(|index| (format!("key-{index}"), serde_json::json!("value")))
        .collect();

    let rows: Vec<Row> = vec![
        Row {
            case: "an operator outside ComparisonOperator's value set, nested in a structure",
            path: "/listStreams",
            request: serde_json::json!({
                "StreamNameCondition": {
                    "ComparisonOperator": "EQUALS",
                    "ComparisonValue": "probe"
                }
            }),
            clause: "Value at 'streamNameCondition.comparisonOperator' failed to satisfy \
                     constraint: Member must satisfy enum value set: [BEGINS_WITH]"
                .to_string(),
            rejected: "EQUALS".to_string(),
        },
        Row {
            case: "the same value set on the signaling-channel listing",
            path: "/listSignalingChannels",
            request: serde_json::json!({
                "ChannelNameCondition": {
                    "ComparisonOperator": "CONTAINS",
                    "ComparisonValue": "probe"
                }
            }),
            clause: "Value at 'channelNameCondition.comparisonOperator' failed to satisfy \
                     constraint: Member must satisfy enum value set: [BEGINS_WITH]"
                .to_string(),
            rejected: "CONTAINS".to_string(),
        },
        Row {
            case: "an APIName outside its value set",
            path: "/getDataEndpoint",
            request: serde_json::json!({ "StreamName": "probe", "APIName": "GET_EVERYTHING" }),
            clause: "at 'aPIName' failed to satisfy constraint: Member must satisfy enum value \
                     set: ["
                .to_string(),
            rejected: "GET_EVERYTHING".to_string(),
        },
        Row {
            case: "a MaxResults below the modeled range minimum of 1",
            path: "/listStreams",
            request: serde_json::json!({ "MaxResults": 0 }),
            clause: "Value at 'maxResults' failed to satisfy constraint: Member must have value \
                     greater than or equal to 1"
                .to_string(),
            rejected: "0".to_string(),
        },
        Row {
            case: "a MaxResults above the modeled range maximum of 10000",
            path: "/listSignalingChannels",
            request: serde_json::json!({ "MaxResults": 10_001 }),
            clause: "Value at 'maxResults' failed to satisfy constraint: Member must have value \
                     less than or equal to 10000"
                .to_string(),
            rejected: "10001".to_string(),
        },
        Row {
            case: "a StreamName past its length maximum of 256",
            path: "/describeStream",
            request: serde_json::json!({ "StreamName": over_long_name.clone() }),
            clause: "Value at 'streamName' failed to satisfy constraint: Member must have length \
                     less than or equal to 256"
                .to_string(),
            rejected: over_long_name.clone(),
        },
        Row {
            case: "a StreamARN that is not an ARN at all",
            path: "/listTagsForStream",
            request: serde_json::json!({ "StreamARN": "not-an-arn" }),
            clause: "Value at 'streamARN' failed to satisfy constraint: Member must satisfy \
                     regular expression pattern: arn:[a-z\\d-]+:kinesisvideo:[a-z0-9-]+:[0-9]+:\
                     [a-z]+/[a-zA-Z0-9_.-]+/[0-9]+"
                .to_string(),
            rejected: "not-an-arn".to_string(),
        },
        Row {
            case: "an empty TagKeyList, below the list's length minimum of 1",
            path: "/UntagResource",
            request: serde_json::json!({
                "ResourceARN": "arn:aws:kinesisvideo:us-east-1:000000000000:stream/probe/1700000000000",
                "TagKeyList": []
            }),
            clause: "Value at 'tagKeyList' failed to satisfy constraint: Member must have length \
                     greater than or equal to 1"
                .to_string(),
            rejected: "0 items".to_string(),
        },
        Row {
            case: "a tag map past the map's length maximum of 50 entries",
            path: "/tagStream",
            request: serde_json::json!({
                "StreamName": "probe",
                "Tags": serde_json::Value::Object(over_long_tags.clone())
            }),
            clause: "Value at 'tags' failed to satisfy constraint: Member must have length less \
                     than or equal to 50"
                .to_string(),
            rejected: "51 entries".to_string(),
        },
    ];

    for row in &rows {
        let case = row.case;
        let (status, body, error_type) = call(&router, row.path, row.request.clone()).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_ARGUMENT,
            "case {case:?}: expected {INVALID_ARGUMENT}, got {body:?}"
        );
        assert!(
            error_type.starts_with(INVALID_ARGUMENT),
            "case {case:?}: expected an x-amzn-errortype naming {INVALID_ARGUMENT}, got \
             {error_type:?}"
        );
        let stated = message(&body);
        assert!(
            body.get("message").is_none(),
            "case {case:?}: the text must be carried under the modelled member alone, got \
             {body:?}"
        );
        assert!(
            stated.starts_with("1 validation error detected: "),
            "case {case:?}: expected the one-error envelope, got {stated:?}"
        );
        assert!(
            stated.contains(&row.clause),
            "case {case:?}: expected a clause stating {:?}, got {stated:?}",
            row.clause
        );
        assert!(
            !stated.contains(&row.rejected),
            "case {case:?}: the clause must quote no rejected value, but {:?} appears in \
             {stated:?}",
            row.rejected
        );
        assert!(
            !stated.contains("Value '"),
            "case {case:?}: the clause must quote no value at all, got {stated:?}"
        );
    }
}

/// RULE: a member the request did not carry is still stated as the `null` it was — the
/// value half is dropped for a value the request SENT, not for the absence of one.
///
/// The rows differ along which `@required` member is missing and on which operation.
#[tokio::test]
async fn an_absent_required_member_is_stated_as_the_null_it_was() {
    let router = harness();

    // (case, path, request, the expected message in full)
    let rows: Vec<(&str, &str, serde_json::Value, String)> = vec![
        (
            "CreateStream without its required StreamName",
            "/createStream",
            serde_json::json!({ "MediaType": "video/h264" }),
            "1 validation error detected: Value null at 'streamName' failed to satisfy \
             constraint: Member must not be null"
                .to_string(),
        ),
        (
            "CreateSignalingChannel without its required ChannelName",
            "/createSignalingChannel",
            serde_json::json!({ "ChannelType": "SINGLE_MASTER" }),
            "1 validation error detected: Value null at 'channelName' failed to satisfy \
             constraint: Member must not be null"
                .to_string(),
        ),
        (
            "GetDataEndpoint without its required APIName",
            "/getDataEndpoint",
            serde_json::json!({ "StreamName": "probe" }),
            "1 validation error detected: Value null at 'aPIName' failed to satisfy constraint: \
             Member must not be null"
                .to_string(),
        ),
    ];

    for (case, path, request, expected) in &rows {
        let (status, body, _) = call(&router, path, request.clone()).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_ARGUMENT,
            "case {case:?}: expected {INVALID_ARGUMENT}, got {body:?}"
        );
        let stated = message(&body);
        assert_eq!(
            &stated, expected,
            "case {case:?}: expected {expected:?}, got {stated:?}"
        );
    }
}
