// SPDX-License-Identifier: CC0-1.0

//! How this control plane WORDS a request that breaks a modelled constraint, end to end
//! over HTTP.
//!
//! `bedrock-agentcore-control` is one endpoint in front of several AgentCore services, one
//! per resource family, and they do not word a constraint refusal alike. Every row below
//! asserts one of the four rules `src/service/validation.rs` states, over a SET of
//! operations, members, constraint classes and request locations rather than over the one
//! request a report happened to cite:
//!
//! 1. STATUS AND CODE. Every refusal is `400` with `x-amzn-errortype:
//!    ValidationException`, the shape every operation of this service declares for input
//!    outside its constraints.
//! 2. THE VALUE. The families reached under `/memories`, `/configuration-bundles` and
//!    `/identities` state the member and the requirement ALONE; every other family quotes
//!    the value the request carried. The rows span all three value-less families and four
//!    of the others, with the same violation classes on both sides — so what differs is the
//!    family, not the member, the constraint or where the value travelled.
//! 3. THE PATTERN. A `@pattern` requirement is quoted without the `^` / `$` anchors the
//!    model spells it with. Each row carries the modelled expression and the clause is
//!    checked against the unanchored form, so the row cannot pass by quoting the model's.
//! 4. A COLLECTION'S ELEMENTS. A violation on the ELEMENTS of a list is stated ONCE, at the
//!    list member — never at an element position — with the element shape's WHOLE
//!    constraint set, not just the constraint that failed.
//!
//! Sources: `smithy_query(query="operation", operation="…")` for every route, member and
//! constraint asserted, and `smithy_query(query="shape",
//! shape="ResourceOauth2ReturnUrlListType,TagKeyList")` for the two element constraint sets
//! the collection rows expect.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol::EmulatedService;

/// The region the rows sign for.
const REGION: &str = "us-east-1";

/// A `memoryId` inside `MemoryId`'s pattern and its 12-character minimum, for a row whose
/// offending member is somewhere else.
const VALID_MEMORY_ID: &str = "amemory12345-abcdefghij";

/// A `WorkloadIdentityNameType` inside its `@pattern` and its 3..255 `@length`.
const VALID_IDENTITY_NAME: &str = "probe.identity-1";

/// A `ResourceOauth2ReturnUrlType` inside its `@pattern` and its 1..2048 `@length`.
const VALID_RETURN_URL: &str = "https://example.com/oauth2/callback";

/// The element constraint set `ResourceOauth2ReturnUrlListType`'s member declares, in the
/// order a clause states it.
///
/// Source: `smithy_query(query="shape", shape="ResourceOauth2ReturnUrlListType")` — the
/// member carries `@length(min: 1, max: 2048)` and
/// `@pattern("^\\w+:(\\/?\\/?)[^\\s]+$")`.
const RETURN_URL_ELEMENT_SET: &str = "Member must have length less than or equal to 2048, \
                                      Member must have length greater than or equal to 1, \
                                      Member must satisfy regular expression pattern: \
                                      \\w+:(\\/?\\/?)[^\\s]+";

/// The element constraint set `TagKeyList`'s member declares, in the same order.
///
/// Source: `smithy_query(query="shape", shape="TagKeyList")` — the member carries
/// `@length(min: 1, max: 128)` and `@pattern("^[a-zA-Z0-9\\s._:/=+@-]*$")`.
const TAG_KEY_ELEMENT_SET: &str = "Member must have length less than or equal to 128, \
                                   Member must have length greater than or equal to 1, \
                                   Member must satisfy regular expression pattern: \
                                   [a-zA-Z0-9\\s._:/=+@-]*";

fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::default();
    router.register_service(EmulatedService::new(engine));
    router
}

/// A SigV4 `Authorization` header for this service: what the router routes on.
fn authorization() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/bedrock-agentcore/\
         aws4_request, SignedHeaders=host, Signature=abc"
    )
}

/// One answer: the status, the `x-amzn-errortype` header and the `message` the body carries.
struct Answer {
    status: u16,
    error_type: String,
    message: String,
}

async fn send(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<&Value>) -> Answer {
    let payload = body.map_or_else(Bytes::new, |value| {
        Bytes::from(serde_json::to_vec(value).expect("the request body serializes"))
    });
    let mut request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", authorization())
        .header("host", "bedrock-agentcore-control.us-east-1.amazonaws.com");
    if body.is_some() {
        request = request.header("content-type", "application/json");
    }
    let request = request.body(payload).expect("a well-formed request");
    let response = router.dispatch(request).await;
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    Answer {
        status: response.status().as_u16(),
        error_type,
        message: parsed
            .pointer("/message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    }
}

/// One refused request: how it is sent, and the clause the service must answer with.
struct Row {
    /// What this row is a case of, named in every assertion it makes.
    case: &'static str,
    method: &'static str,
    uri: String,
    body: Option<Value>,
    /// The member path the clause names.
    member: &'static str,
    /// The requirement the clause states.
    requirement: String,
    /// What the request put in that member, which rule 2 either quotes or does not.
    submitted: &'static str,
}

/// `@pattern` as a requirement reads, from the expression WITHOUT its whole-value anchors.
///
/// The row carries the MODELLED (anchored) expression, so a clause that quoted the model's
/// spelling — the defect this suite pins — fails here rather than passing.
fn pattern_requirement(modelled: &str) -> String {
    let unanchored = modelled
        .strip_prefix('^')
        .and_then(|rest| rest.strip_suffix('$'))
        .expect("every modelled pattern below is anchored at both ends");
    format!("Member must satisfy regular expression pattern: {unanchored}")
}

/// Assert the status and the error code rule 1 states.
fn assert_refused(answer: &Answer, case: &str) {
    assert_eq!(
        (answer.status, answer.error_type.as_str()),
        (400, "ValidationException"),
        "case {case:?}: a value outside its modelled constraint is refused 400 \
         ValidationException, got {} {:?} with {:?}",
        answer.status,
        answer.error_type,
        answer.message
    );
}

/// RULE 2 (value-less half) and RULE 3: the families served by the backends that state no
/// value name the member and the requirement alone — whatever the operation, the member, the
/// constraint class, or where in the request the value travelled.
#[tokio::test]
async fn a_value_less_family_states_the_member_and_the_requirement_alone() {
    let router = router();
    let rows = vec![
        Row {
            case: "CreateMemory name, a body member's @pattern",
            method: "POST",
            uri: "/memories/create".to_string(),
            body: Some(json!({"name": "probe-memory-1", "eventExpiryDuration": 30})),
            member: "name",
            requirement: pattern_requirement("^[a-zA-Z][a-zA-Z0-9_]{0,47}$"),
            submitted: "probe-memory-1",
        },
        Row {
            // Not a pattern: the value policy is the family's, not one constraint class's.
            case: "CreateMemory eventExpiryDuration, a body member's @range",
            method: "POST",
            uri: "/memories/create".to_string(),
            body: Some(json!({"name": "probeMemory", "eventExpiryDuration": 999})),
            member: "eventExpiryDuration",
            requirement: "Member must have value less than or equal to 365".to_string(),
            submitted: "999",
        },
        Row {
            // A URI label, not the body: where the value travelled does not decide either.
            case: "GetMemory memoryId, a URI label's @pattern",
            method: "GET",
            uri: "/memories/1notamemory-abcdefghij/details".to_string(),
            body: None,
            member: "memoryId",
            requirement: pattern_requirement("^[a-zA-Z][a-zA-Z0-9-_]{0,99}-[a-zA-Z0-9]{10}$"),
            submitted: "1notamemory-abcdefghij",
        },
        Row {
            case: "UpdateMemory memoryExecutionRoleArn, another operation of the family",
            method: "PUT",
            uri: format!("/memories/{VALID_MEMORY_ID}/update"),
            body: Some(json!({"memoryExecutionRoleArn": "not-an-arn"})),
            member: "memoryExecutionRoleArn",
            requirement: pattern_requirement(
                "^arn:[a-z0-9-\\.]{1,63}:[a-z0-9-\\.]{0,63}:[a-z0-9-\\.]{0,63}:\
                 [a-z0-9-\\.]{0,63}:[^/].{0,1023}$",
            ),
            submitted: "not-an-arn",
        },
        Row {
            case: "CreateConfigurationBundle bundleName, a second family",
            method: "POST",
            uri: "/configuration-bundles/create".to_string(),
            body: Some(json!({"bundleName": "9bundle", "components": {}})),
            member: "bundleName",
            requirement: pattern_requirement("^[a-zA-Z][a-zA-Z0-9_]{0,99}$"),
            submitted: "9bundle",
        },
        Row {
            case: "GetConfigurationBundle bundleId, that family's REST-style route",
            method: "GET",
            uri: "/configuration-bundles/9bundle-abcdefghij".to_string(),
            body: None,
            member: "bundleId",
            requirement: pattern_requirement("^[a-zA-Z][a-zA-Z0-9-_]{0,99}-[a-zA-Z0-9]{10}$"),
            submitted: "9bundle-abcdefghij",
        },
        Row {
            case: "CreateWorkloadIdentity name, a third family",
            method: "POST",
            uri: "/identities/CreateWorkloadIdentity".to_string(),
            body: Some(json!({"name": "a!a"})),
            member: "name",
            requirement: pattern_requirement("^[A-Za-z0-9_.-]+$"),
            submitted: "a!a",
        },
        Row {
            case: "CreateWorkloadIdentity name, the same member's @length",
            method: "POST",
            uri: "/identities/CreateWorkloadIdentity".to_string(),
            body: Some(json!({"name": "ab"})),
            member: "name",
            requirement: "Member must have length greater than or equal to 3".to_string(),
            submitted: "ab",
        },
        Row {
            case: "GetConsentPortal consentPortalIdentifier, another resource of the family",
            method: "POST",
            uri: "/identities/GetConsentPortal".to_string(),
            body: Some(json!({"consentPortalIdentifier": "bad portal!"})),
            member: "consentPortalIdentifier",
            requirement: pattern_requirement("^[a-zA-Z0-9\\-_]{1,50}-[A-Za-z0-9]{10}$"),
            submitted: "bad portal!",
        },
        Row {
            // The family decides, not the shape of the URI: this route names no operation.
            case: "GetTokenVault tokenVaultId, the family's non-operation-named route",
            method: "POST",
            uri: "/identities/get-token-vault".to_string(),
            body: Some(json!({"tokenVaultId": "bad vault!"})),
            member: "tokenVaultId",
            requirement: pattern_requirement("^[a-zA-Z0-9\\-_]+$"),
            submitted: "bad vault!",
        },
    ];

    for row in rows {
        let case = row.case;
        let answer = send(&router, row.method, &row.uri, row.body.as_ref()).await;
        assert_refused(&answer, case);
        let expected = format!(
            "1 validation error detected: Value at '{}' failed to satisfy constraint: {}",
            row.member, row.requirement
        );
        assert_eq!(
            answer.message, expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            answer.message
        );
        assert!(
            !answer.message.contains(row.submitted),
            "case {case:?}: this family's clause quotes no value, yet {:?} carries the \
             submitted {:?}",
            answer.message,
            row.submitted
        );
    }
}

/// RULE 2 (the echoing half) and RULE 3: every other family quotes the value the request
/// carried, in the same clause and for the same constraint classes.
#[tokio::test]
async fn every_other_family_quotes_the_value_the_request_carried() {
    let router = router();
    let rows = vec![
        Row {
            case: "ListBrowserProfiles name, a body member's @pattern",
            method: "POST",
            uri: "/browser-profiles".to_string(),
            body: Some(json!({"name": "bad profile name!"})),
            member: "name",
            requirement: pattern_requirement("^[a-zA-Z][a-zA-Z0-9_]{0,47}$"),
            submitted: "bad profile name!",
        },
        Row {
            case: "CreateBrowserProfile name, another operation of that family",
            method: "PUT",
            uri: "/browser-profiles".to_string(),
            body: Some(json!({"name": "9profile"})),
            member: "name",
            requirement: pattern_requirement("^[a-zA-Z][a-zA-Z0-9_]{0,47}$"),
            submitted: "9profile",
        },
        Row {
            case: "GetAgentRuntime agentRuntimeId, a URI label's @pattern",
            method: "GET",
            uri: "/runtimes/9runtime-abcdefghij/".to_string(),
            body: None,
            member: "agentRuntimeId",
            requirement: pattern_requirement("^[a-zA-Z][a-zA-Z0-9_]{0,99}-[a-zA-Z0-9]{10}$"),
            submitted: "9runtime-abcdefghij",
        },
        Row {
            case: "GetHarness harnessId, a third family",
            method: "GET",
            uri: "/harnesses/1notaharness-abcdefghij".to_string(),
            body: None,
            member: "harnessId",
            requirement: pattern_requirement("^[a-zA-Z][a-zA-Z0-9_]{0,39}-[a-zA-Z0-9]{10}$"),
            submitted: "1notaharness-abcdefghij",
        },
        Row {
            case: "GetPaymentManager paymentManagerId, a fourth family",
            method: "GET",
            uri: "/payments/managers/BADMANAGER-abcdefghij".to_string(),
            body: None,
            member: "paymentManagerId",
            requirement: pattern_requirement("^([0-9a-z][-]?){1,100}-[0-9a-z]{10}$"),
            submitted: "BADMANAGER-abcdefghij",
        },
        Row {
            case: "GetGateway gatewayIdentifier, a fifth family",
            method: "GET",
            uri: "/gateways/BADGATEWAY-abcdefghij/".to_string(),
            body: None,
            member: "gatewayIdentifier",
            requirement: pattern_requirement("^([0-9a-z][-]?){1,100}-[0-9a-z]{10}$"),
            submitted: "BADGATEWAY-abcdefghij",
        },
    ];

    for row in rows {
        let case = row.case;
        let answer = send(&router, row.method, &row.uri, row.body.as_ref()).await;
        assert_refused(&answer, case);
        let expected = format!(
            "1 validation error detected: Value '{}' at '{}' failed to satisfy constraint: {}",
            row.submitted, row.member, row.requirement
        );
        assert_eq!(
            answer.message, expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            answer.message
        );
    }
}

/// RULE 4: a constraint the ELEMENTS of a list break is stated once, at the list member,
/// with the element shape's whole constraint set — however many elements broke it and
/// whichever of its constraints they broke.
#[tokio::test]
async fn a_failing_element_is_stated_once_at_its_list_with_the_whole_element_set() {
    let router = router();
    // (case, the list the request sent, the elements that offend)
    let rows: [(&str, Value); 5] = [
        (
            "one element outside the element @pattern",
            json!(["not a return url"]),
        ),
        (
            "two elements outside it: still ONE clause, at the list",
            json!(["not a return url", "nor is this one"]),
        ),
        (
            "a valid element beside an invalid one",
            json!([VALID_RETURN_URL, "not a return url"]),
        ),
        (
            "an element outside the element @length AND its @pattern",
            json!([""]),
        ),
        (
            "an invalid element at a later position",
            json!([VALID_RETURN_URL, VALID_RETURN_URL, "still not a url"]),
        ),
    ];

    let expected = format!(
        "1 validation error detected: Value at 'allowedResourceOauth2ReturnUrls' failed to \
         satisfy constraint: Member must satisfy constraint: [{RETURN_URL_ELEMENT_SET}]"
    );
    for (case, urls) in rows {
        let body = json!({
            "name": VALID_IDENTITY_NAME,
            "allowedResourceOauth2ReturnUrls": urls,
        });
        let answer = send(
            &router,
            "POST",
            "/identities/UpdateWorkloadIdentity",
            Some(&body),
        )
        .await;
        assert_refused(&answer, case);
        assert_eq!(
            answer.message, expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            answer.message
        );
        assert!(
            !answer.message.contains(".member"),
            "case {case:?}: a clause names the list member, never an element position, got \
             {:?}",
            answer.message
        );
    }
}

/// RULE 4 again, in a family that DOES quote the value (rule 2): the clause is still stated
/// at the list member with the element shape's whole set, and the value it quotes is the
/// list the caller sent rather than the one element that offended.
#[tokio::test]
async fn an_echoing_familys_list_clause_quotes_the_list_the_caller_sent() {
    let router = router();
    let arn = format!(
        "arn:aws:bedrock-agentcore:{REGION}:000000000000:memory%2F{VALID_MEMORY_ID}"
    );
    let answer = send(
        &router,
        "DELETE",
        &format!("/tags/{arn}?tagKeys=good.key&tagKeys=bad%21key"),
        None,
    )
    .await;
    let case = "UntagResource tagKeys, a query list whose element breaks its @pattern";
    assert_refused(&answer, case);
    let clause = format!(
        "Value '[good.key, bad!key]' at 'tagKeys' failed to satisfy constraint: Member must \
         satisfy constraint: [{TAG_KEY_ELEMENT_SET}]"
    );
    assert!(
        answer.message.contains(&clause),
        "case {case:?}: expected the clause {clause:?}, got {:?}",
        answer.message
    );
    assert!(
        !answer.message.contains(".member"),
        "case {case:?}: a clause names the list member, never an element position, got {:?}",
        answer.message
    );
}

/// An ABSENT `@required` member has no value to quote OR to suppress, so both policies
/// report it as the null it was.
#[tokio::test]
async fn an_absent_required_member_is_reported_as_the_null_it_was() {
    let router = router();
    // (case, method, uri, the member the request omits)
    let rows: [(&str, &str, &str, &str); 3] = [
        (
            "CreateWorkloadIdentity name, in a value-less family",
            "POST",
            "/identities/CreateWorkloadIdentity",
            "name",
        ),
        (
            "CreateMemory name, in another value-less family",
            "POST",
            "/memories/create",
            "name",
        ),
        (
            "CreateBrowserProfile name, in a family that quotes values",
            "PUT",
            "/browser-profiles",
            "name",
        ),
    ];

    for (case, method, uri, member) in rows {
        let answer = send(&router, method, uri, Some(&json!({}))).await;
        assert_refused(&answer, case);
        let expected = format!(
            "1 validation error detected: Value null at '{member}' failed to satisfy \
             constraint: Member must not be null"
        );
        assert_eq!(
            answer.message, expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            answer.message
        );
    }
}

/// A request that satisfies every modelled constraint is NOT refused here: the wording seam
/// only words what the framework's own walk found, so a valid request reaches its operation.
#[tokio::test]
async fn a_request_inside_every_constraint_reaches_its_operation() {
    let router = router();
    let answer = send(
        &router,
        "POST",
        "/identities/CreateWorkloadIdentity",
        Some(&json!({
            "name": VALID_IDENTITY_NAME,
            "allowedResourceOauth2ReturnUrls": [VALID_RETURN_URL],
        })),
    )
    .await;
    assert!(
        answer.status != 400 && answer.error_type != "ValidationException",
        "case \"a CreateWorkloadIdentity inside every constraint\": expected no validation \
         refusal, got {} {:?} with {:?}",
        answer.status,
        answer.error_type,
        answer.message
    );
}
