// SPDX-License-Identifier: CC0-1.0

//! How this service reports a `@pattern` constraint it refused a request for.
//!
//! The rule under test: a constraint-violation message states a `@pattern` requirement
//! as the regular EXPRESSION a value must satisfy, without the leading `^` and trailing
//! `$` a model writes to turn Smithy's unanchored find into a whole-string match — while
//! the MATCH itself still enforces the whole value. Everything else about the refusal is
//! the framework's: HTTP 400, `x-amzn-errortype: ValidationException`, the
//! `N validation error(s) detected: Value '<sent>' at '<member path>' failed to satisfy
//! constraint: <requirement>` envelope, and the untouched wording of every other
//! constraint class.
//!
//! The recorded fixture (a feature group name of `not a valid name!!` on each operation)
//! is ONE row of the table below. The rows differ along what the rule generalises over:
//! which operation and member carries the value, how the value fails the expression
//! (a disallowed character, a leading separator, a trailing separator, too many
//! characters, a valid name with trailing junk), where in a list the offending element
//! sits, and WHICH pattern-constrained shape the member targets.
//!
//! Every case goes through the real front door — the multi-service router, the restJson1
//! protocol, the modelled constraint validation — so what is asserted is the wire
//! behaviour a client sees.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::{http, MultiServiceRouter};

use service_com_amazonaws_sagemakerfeaturestoreruntime_amazonsagemakerfeaturestoreruntime::EmulatedService;

/// `FeatureGroupNameOrArn`'s modelled `@pattern`, as this service REPORTS it: the
/// expression without the anchors the model spells it with. Source:
/// `smithy_query(query="shape", shape="FeatureGroupNameOrArn")` —
/// `^(arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:feature-group/)?([a-zA-Z0-9]([-_]*[a-zA-Z0-9]){0,63})$`.
const GROUP_PATTERN: &str = r"(arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:feature-group/)?([a-zA-Z0-9]([-_]*[a-zA-Z0-9]){0,63})";

/// `FeatureName`'s modelled `@pattern`, likewise as reported. Source:
/// `smithy_query(query="shape", shape="FeatureName")` —
/// `^[a-zA-Z0-9]([-_]*[a-zA-Z0-9]){0,63}$`.
const FEATURE_NAME_PATTERN: &str = "[a-zA-Z0-9]([-_]*[a-zA-Z0-9]){0,63}";

/// A booted emulator holding this service only.
struct Emulator {
    router: MultiServiceRouter,
}

/// One refusal as the wire carries it.
struct Reply {
    status: u16,
    /// restJson1 resolves the error shape from `x-amzn-errortype`.
    errortype: String,
    /// The error body's `message`, or the empty string when the body carries none.
    message: String,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn send(&self, method: &str, uri: &str, body: Option<Value>) -> Reply {
        // The SigV4 credential scope is how the router picks the claiming service: the
        // 4th field is this service's signing name.
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20240101/us-east-1/sagemaker/aws4_request, \
                 SignedHeaders=host, Signature=0000",
            )
            .header("host", "featurestore-runtime.sagemaker.us-east-1.amazonaws.com")
            .body(
                body.map(|value| bytes::Bytes::from(value.to_string()))
                    .unwrap_or_default(),
            )
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let errortype = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        let parsed = serde_json::from_slice::<Value>(response.body()).unwrap_or(Value::Null);
        let message = parsed
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        Reply {
            status,
            errortype,
            message,
        }
    }
}

/// The clause a refusal states for `value` at `path` failing `pattern`: the framework's
/// envelope over the requirement this service reports.
fn expected_message(value: &str, path: &str, pattern: &str) -> String {
    format!(
        "1 validation error detected: Value '{value}' at '{path}' failed to satisfy constraint: \
         Member must satisfy regular expression pattern: {pattern}"
    )
}

/// A valid record body for the write operations, so the only constraint a case violates
/// is the one it is about.
fn a_record() -> Value {
    json!([
        { "FeatureName": "customer_id", "ValueAsString": "C1" },
        { "FeatureName": "event_time", "ValueAsString": "2024-01-01T00:00:00Z" }
    ])
}

/// One table row: the request to send and the clause it must be refused with.
struct Case {
    what: &'static str,
    method: &'static str,
    uri: String,
    body: Option<Value>,
    /// The value the clause echoes back, decoded as the service read it.
    value: String,
    /// The member path the clause names.
    path: String,
    /// The expression the clause reports.
    pattern: &'static str,
}

/// The rows: every bound operation of this service, each carrying a differently-shaped
/// value outside a pattern-constrained member, plus two rows whose member targets
/// `FeatureName` instead of `FeatureGroupNameOrArn`.
fn cases() -> Vec<Case> {
    // 65 alphanumerics: inside `FeatureGroupNameOrArn`'s `@length` max of 150, outside
    // the `{0,63}` repetition of its expression, so the pattern is the only constraint
    // this value fails.
    let too_long_for_the_expression = "a".repeat(65);
    vec![
        Case {
            what: "GetRecord, a name with spaces and punctuation (the recorded fixture)",
            method: "GET",
            uri: "/FeatureGroup/not%20a%20valid%20name!!?RecordIdentifierValueAsString=1"
                .to_string(),
            body: None,
            value: "not a valid name!!".to_string(),
            path: "featureGroupName".to_string(),
            pattern: GROUP_PATTERN,
        },
        Case {
            what: "DeleteRecord, a name ending in a separator",
            method: "DELETE",
            uri: "/FeatureGroup/bad_name_?RecordIdentifierValueAsString=1&EventTime=2024-01-01T00:00:00Z"
                .to_string(),
            body: None,
            value: "bad_name_".to_string(),
            path: "featureGroupName".to_string(),
            pattern: GROUP_PATTERN,
        },
        Case {
            what: "PutRecord, a name starting with a separator",
            method: "PUT",
            uri: "/FeatureGroup/-leading-dash".to_string(),
            body: Some(json!({ "Record": a_record() })),
            value: "-leading-dash".to_string(),
            path: "featureGroupName".to_string(),
            pattern: GROUP_PATTERN,
        },
        Case {
            what: "UpdateRecord, a name longer than the expression admits",
            method: "POST",
            uri: format!("/FeatureGroup/{too_long_for_the_expression}/Record"),
            body: Some(json!({
                "RecordIdentifierValueAsString": "C1",
                "Features": a_record(),
            })),
            value: too_long_for_the_expression.clone(),
            path: "featureGroupName".to_string(),
            pattern: GROUP_PATTERN,
        },
        Case {
            what: "ListRecords, a valid name with trailing junk",
            method: "POST",
            uri: "/FeatureGroup/valid-name!!extra/ListRecords".to_string(),
            body: Some(json!({})),
            value: "valid-name!!extra".to_string(),
            path: "featureGroupName".to_string(),
            pattern: GROUP_PATTERN,
        },
        Case {
            what: "BatchGetRecord, the name of the first identifier",
            method: "POST",
            uri: "/BatchGetRecord".to_string(),
            body: Some(json!({
                "Identifiers": [
                    {
                        "FeatureGroupName": "not a valid name!!",
                        "RecordIdentifiersValueAsString": ["C1"],
                    }
                ]
            })),
            value: "not a valid name!!".to_string(),
            path: "identifiers.1.member.featureGroupName".to_string(),
            pattern: GROUP_PATTERN,
        },
        Case {
            what: "BatchGetRecord, the name of the SECOND identifier",
            method: "POST",
            uri: "/BatchGetRecord".to_string(),
            body: Some(json!({
                "Identifiers": [
                    {
                        "FeatureGroupName": "customers",
                        "RecordIdentifiersValueAsString": ["C1"],
                    },
                    {
                        "FeatureGroupName": "two words",
                        "RecordIdentifiersValueAsString": ["C2"],
                    }
                ]
            })),
            value: "two words".to_string(),
            path: "identifiers.2.member.featureGroupName".to_string(),
            pattern: GROUP_PATTERN,
        },
        Case {
            what: "BatchWriteRecord, the name of the first entry",
            method: "POST",
            uri: "/BatchWriteRecord".to_string(),
            body: Some(json!({
                "Entries": [
                    {
                        "FeatureGroupName": "not a valid name!!",
                        "Record": a_record(),
                    }
                ]
            })),
            value: "not a valid name!!".to_string(),
            path: "entries.1.member.featureGroupName".to_string(),
            pattern: GROUP_PATTERN,
        },
        Case {
            what: "BatchGetRecord, a requested feature name (a different pattern shape)",
            method: "POST",
            uri: "/BatchGetRecord".to_string(),
            body: Some(json!({
                "Identifiers": [
                    {
                        "FeatureGroupName": "customers",
                        "RecordIdentifiersValueAsString": ["C1"],
                        "FeatureNames": ["not a feature!"],
                    }
                ]
            })),
            value: "not a feature!".to_string(),
            path: "identifiers.1.member.featureNames.1.member".to_string(),
            pattern: FEATURE_NAME_PATTERN,
        },
        Case {
            what: "PutRecord, a feature name inside the record (a different pattern shape)",
            method: "PUT",
            uri: "/FeatureGroup/customers".to_string(),
            body: Some(json!({
                "Record": [
                    { "FeatureName": "customer_id", "ValueAsString": "C1" },
                    { "FeatureName": "bad name", "ValueAsString": "x" }
                ]
            })),
            value: "bad name".to_string(),
            path: "record.2.member.featureName".to_string(),
            pattern: FEATURE_NAME_PATTERN,
        },
    ]
}

/// Every pattern-constrained member of every operation reports its expression WITHOUT
/// the anchors the model spells it with — and nothing else about the refusal changes.
#[tokio::test]
async fn a_pattern_violation_reports_the_expression_without_its_anchors() {
    let emulator = Emulator::new();

    for case in cases() {
        let Case {
            what,
            method,
            uri,
            body,
            value,
            path,
            pattern,
        } = case;
        let reply = emulator.send(method, &uri, body).await;

        assert_eq!(
            reply.status, 400,
            "case {what:?}: expected HTTP 400, got {} with {:?}",
            reply.status, reply.message
        );
        assert_eq!(
            reply.errortype, "ValidationException",
            "case {what:?}: expected x-amzn-errortype ValidationException, got {:?}",
            reply.errortype
        );
        let expected = expected_message(&value, &path, pattern);
        assert_eq!(
            reply.message, expected,
            "case {what:?}: expected {expected:?}, got {:?}",
            reply.message
        );
        // The two characters this issue is about, asserted independently of the whole
        // sentence: the reported requirement neither opens with `^` nor ends with `$`.
        let reported = reply
            .message
            .rsplit("regular expression pattern: ")
            .next()
            .unwrap_or_default();
        assert!(
            !reported.starts_with('^') && !reported.ends_with('$'),
            "case {what:?}: the reported expression carries an anchor: {reported:?}"
        );
    }
}

/// Dropping the anchors from the REPORTED text does not drop them from the MATCH: a
/// value that satisfies the expression only as a substring is still refused, and a value
/// the whole-string match accepts is not refused at all.
#[tokio::test]
async fn the_match_still_demands_the_whole_value() {
    let emulator = Emulator::new();

    // Each row satisfies `([a-zA-Z0-9]([-_]*[a-zA-Z0-9]){0,63})` SOMEWHERE inside it, so
    // an unanchored match would accept it; the anchored match the model declares must
    // not. The last column is the value the clause echoes back.
    let only_as_a_substring = [
        ("trailing junk", "valid-name!!extra", "valid-name!!extra"),
        ("leading junk", "!!extra-valid-name", "!!extra-valid-name"),
        (
            "junk on both sides",
            "%20customers%20",
            " customers ",
        ),
        ("an embedded newline", "custo%0Amers", "custo\nmers"),
    ];
    for (case, in_uri, echoed) in only_as_a_substring {
        let reply = emulator
            .send(
                "GET",
                &format!("/FeatureGroup/{in_uri}?RecordIdentifierValueAsString=1"),
                None,
            )
            .await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: a value matching only as a substring must still be refused, got \
             {} with {:?}",
            reply.status, reply.message
        );
        let expected = expected_message(echoed, "featureGroupName", GROUP_PATTERN);
        assert_eq!(
            reply.message, expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            reply.message
        );
    }

    // And the forms the whole-string match accepts are not refused: a bare name, a name
    // with the separators the expression allows, and the optional ARN prefix.
    let inside_the_pattern = [
        ("a bare name", "customers"),
        ("separators the expression allows", "customer-events_2024"),
        ("a 64-character name", "a1234567890123456789012345678901234567890123456789012345678901b"),
        (
            "the optional ARN prefix",
            "arn:aws:sagemaker:us-east-1:123456789012:feature-group/customers",
        ),
    ];
    for (case, name) in inside_the_pattern {
        let reply = emulator
            .send(
                "GET",
                &format!("/FeatureGroup/{name}?RecordIdentifierValueAsString=1"),
                None,
            )
            .await;
        assert!(
            !reply.message.contains("regular expression pattern"),
            "case {case:?}: a value inside the pattern must not be refused for it, got {} with \
             {:?}",
            reply.status,
            reply.message
        );
    }
}

/// The rule is confined to the `@pattern` class: every other constraint keeps the
/// framework's own requirement wording, and the envelope keeps the framework's count.
#[tokio::test]
async fn other_constraint_classes_keep_the_frameworks_wording() {
    let emulator = Emulator::new();

    // `ListRecordsRequest$MaxResults` is `@range(min: 1, max: 100)`
    // (`smithy_query(query="shape", shape="ListRecordsRequest")`), on a group name that
    // is inside its pattern, so the only violation is the numeric bound.
    let reply = emulator
        .send(
            "POST",
            "/FeatureGroup/customers/ListRecords",
            Some(json!({ "MaxResults": 1000 })),
        )
        .await;
    assert_eq!(
        reply.status, 400,
        "a MaxResults above the modelled range is refused, got {} with {:?}",
        reply.status, reply.message
    );
    assert!(
        reply
            .message
            .contains("Member must have value less than or equal to 100")
            && !reply.message.contains("regular expression pattern"),
        "case \"MaxResults above the range\": expected the framework's numeric-bound \
         requirement, got {:?}",
        reply.message
    );

    // An absent `@required` member: likewise untouched, and still one error.
    let reply = emulator
        .send("POST", "/BatchGetRecord", Some(json!({})))
        .await;
    assert_eq!(
        reply.status, 400,
        "an absent required member is refused, got {} with {:?}",
        reply.status, reply.message
    );
    assert!(
        reply.message.starts_with("1 validation error detected: ")
            && reply.message.contains("Member must not be null"),
        "case \"an absent required member\": expected the framework's null requirement, got \
         {:?}",
        reply.message
    );

    // Two violations in one request still take the framework's plural envelope, with the
    // pattern clause among them reported unanchored.
    let reply = emulator
        .send(
            "POST",
            "/FeatureGroup/bad_name_/ListRecords",
            Some(json!({ "MaxResults": 0 })),
        )
        .await;
    assert!(
        reply.message.starts_with("2 validation errors detected: ")
            && reply
                .message
                .contains(&format!("regular expression pattern: {GROUP_PATTERN}"))
            && reply
                .message
                .contains("Member must have value greater than or equal to 1"),
        "case \"two violations in one request\": expected both clauses under the plural \
         envelope, got {:?}",
        reply.message
    );
}
