// SPDX-License-Identifier: CC0-1.0

//! How the `/2017-03-31/tags/{Resource}` operations refuse a `Resource` label the
//! modelled `@pattern` does not admit, and which resource ARNs that pattern admits.
//!
//! Two rules are under test, both about the ONE `TaggableResource` constraint the three
//! tag operations share (`ListTags` `GET`, `TagResource` `POST`, `UntagResource`
//! `DELETE`, all bound to `/2017-03-31/tags/{Resource}`):
//!
//! 1. **The rendering.** A constraint-violation message is BUILT FROM THE REQUEST — the
//!    offending value, the member path and the member's own requirement — and a
//!    `@pattern` requirement states its expression WITHOUT the `^`/`$` anchors a
//!    whole-value Smithy pattern wraps it in. So every refusal below reads
//!    `1 validation error detected: Value '<value>' at '<member>' failed to satisfy
//!    constraint: Member must satisfy regular expression pattern: <expression>`, where
//!    `<expression>` is the model's pattern with that outer pair dropped and nothing else
//!    changed. The rule is a property of the RENDERING, not of one member, so it is
//!    checked on the `Resource` label of all three tag operations AND on a second member
//!    of a different operation (`CreateFunction`'s `Role`).
//!
//! 2. **The accepted set.** The pattern is the acceptance predicate for the label, so
//!    every Lambda resource ARN it admits — a function, optionally qualified, a LAYER, a
//!    code-signing configuration, an event source mapping, a capacity provider and a
//!    network connector — reaches the operation's handler and is answered from state,
//!    while anything outside it is refused at the boundary. A layer ARN is the row this
//!    suite exists for: before the fix the shared pattern carried no `layer:` alternative,
//!    so tagging, listing tags on, or untagging a Lambda layer was refused 400 at the
//!    boundary. The alternative's source is the constraint AWS itself echoes for a refused
//!    `Resource` (the `simparity-wire-aws-v1` fixture `lambda-microvms-tag-error-paths`,
//!    whose recorded message quotes `…|layer:([a-zA-Z0-9-_]+)|…`), corroborated by the
//!    sibling Lambda MicroVMs model, whose `TaggableResource` carries the same
//!    alternative (`smithy_query(query="shape", shape="TaggableResource")` on it).
//!
//! The rows differ along what the rules generalise over: the resource TYPE the ARN names,
//! the PARTITION and region it carries, the FORM of the resource id (the whole character
//! class the alternative admits, both ends of a published length bound and one past it),
//! the HTTP METHOD (so each of the three operations is exercised), and — for the
//! rendering — the MEMBER whose pattern was broken.

use std::collections::BTreeMap;
use std::sync::Arc;

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// Where the tag operations are bound (`smithy_query(query="operation",
/// operation="ListTags")`: `GET /2017-03-31/tags/{Resource}`; `TagResource` and
/// `UntagResource` bind the same URI with `POST` and `DELETE`).
const TAGS_URI: &str = "/2017-03-31/tags";

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// A SigV4 credential scope naming this service's signing name (`lambda`) in
/// `us-east-1`, which is what routes a request here. Nothing in it is verified: the
/// harness turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/lambda/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// The region the scope above names.
const REGION: &str = "us-east-1";

/// The account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The `Resource` requirement as AWS states it: the model's `TaggableResource` pattern
/// (`smithy_query(query="shape", shape="TaggableResource")`) with the anchors the
/// whole-value expression is wrapped in dropped, and nothing else changed.
const RESOURCE_EXPRESSION: &str = r"arn:(aws[a-zA-Z-]*):lambda:(eusc-)?[a-z]{2}((-gov)|(-iso([a-z]?)))?-[a-z]+-\d{1}:\d{12}:(function:[a-zA-Z0-9-_]+(:(\$LATEST|[a-zA-Z0-9-_]+))?|layer:([a-zA-Z0-9-_]+)|code-signing-config:csc-[a-z0-9]{17}|event-source-mapping:[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}|(capacity-provider|network-connector):[a-zA-Z0-9-_]{1,64})";

/// `CreateFunction`'s `Role` requirement, stated the same way: the model's `RoleArn`
/// pattern (`smithy_query(query="shape", shape="RoleArn")`,
/// `^arn:(aws[a-zA-Z-]*)?:iam::\d{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+$`) without its outer
/// pair. A second member, of a second operation, so the rendering is pinned as a rule
/// rather than as one clause of one message.
const ROLE_EXPRESSION: &str = r"arn:(aws[a-zA-Z-]*)?:iam::\d{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+";

/// The longest `capacity-provider` / `network-connector` id the pattern admits
/// (`[a-zA-Z0-9-_]{1,64}`).
const MAX_PROVIDER_ID_LENGTH: usize = 64;

/// What a dispatched request answered.
struct Answer {
    status: u16,
    headers: http::HeaderMap,
    body: bytes::Bytes,
}

impl Answer {
    /// The body as JSON, or `Value::Null` for an empty body.
    fn json(&self) -> Value {
        if self.body.is_empty() {
            return Value::Null;
        }
        serde_json::from_slice(&self.body).unwrap_or_else(|error| {
            panic!(
                "expected a JSON body, got {error}: {}",
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    /// A `String` member of the JSON body.
    fn string(&self, member: &str) -> Option<String> {
        self.json()
            .get(member)
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    /// The wire error code (`__type`, or the `x-amzn-errortype` header), with any
    /// namespace or trailing URL a client does not match on removed.
    fn error_code(&self) -> String {
        self.string("__type")
            .or_else(|| {
                self.headers
                    .get("x-amzn-errortype")
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| String::from_utf8_lossy(&self.body).to_string())
            .rsplit(['#', '/'])
            .next()
            .unwrap_or_default()
            .to_string()
    }

    /// The error message member, however the envelope spells it.
    fn message(&self) -> String {
        self.string("message")
            .or_else(|| self.string("Message"))
            .unwrap_or_else(|| self.text())
    }

    /// The body text, for a failure message.
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }
}

/// One service mounted on its own engine, reached the way the production router reaches
/// it.
fn router_on(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router
}

/// Dispatch one JSON request.
async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: &Value) -> Answer {
    let payload = if body.is_null() {
        Vec::new()
    } else {
        serde_json::to_vec(body).expect("the test request body must serialize")
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("http://lambda.us-east-1.amazonaws.com{uri}"))
        .header("host", "lambda.us-east-1.amazonaws.com")
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(payload))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body,
    }
}

/// The three operations bound to `/2017-03-31/tags/{Resource}`: the wire name, the HTTP
/// method, the query string each needs for its own required members, and the request
/// body. Every one carries whatever its OTHER required members need, so the only
/// constraint a row can break is the `Resource` label's own.
struct TagOperation {
    name: &'static str,
    method: &'static str,
    query: &'static str,
    body: fn() -> Value,
}

/// `ListTags` reads, `TagResource` carries the tags to add, `UntagResource` carries the
/// keys to remove as its `@httpQuery("tagKeys")` member.
const TAG_OPERATIONS: &[TagOperation] = &[
    TagOperation {
        name: "ListTags",
        method: "GET",
        query: "",
        body: no_body,
    },
    TagOperation {
        name: "TagResource",
        method: "POST",
        query: "",
        body: tag_body,
    },
    TagOperation {
        name: "UntagResource",
        method: "DELETE",
        query: "?tagKeys=env",
        body: no_body,
    },
];

/// The `Tags` member `TagResource` requires.
fn tag_body() -> Value {
    json!({ "Tags": { "env": "prod" } })
}

/// No body at all, for the two operations that take none.
fn no_body() -> Value {
    Value::Null
}

/// Call one tag operation on `resource`.
async fn call_tag_operation(
    router: &MultiServiceRouter,
    operation: &TagOperation,
    resource: &str,
) -> Answer {
    let uri = format!("{TAGS_URI}/{resource}{}", operation.query);
    call(router, operation.method, &uri, &(operation.body)()).await
}

/// `CreateFunction` with `role`, which a row makes well formed or malformed.
async fn create_function_with_role(
    router: &MultiServiceRouter,
    name: &str,
    role: &str,
    tags: &BTreeMap<String, String>,
) -> Answer {
    let package: Vec<u8> = (0..64u16)
        .map(|byte| u8::try_from(byte % 251).unwrap_or(0))
        .collect();
    let mut body = json!({
        "FunctionName": name,
        "Role": role,
        "Runtime": "python3.12",
        "Handler": "index.handler",
        "Publish": false,
        "Code": { "ZipFile": base64::engine::general_purpose::STANDARD.encode(&package) },
    });
    if !tags.is_empty() {
        body["Tags"] = Value::Object(
            tags.iter()
                .map(|(key, value)| (key.clone(), Value::String(value.clone())))
                .collect(),
        );
    }
    call(router, "POST", FUNCTIONS_URI, &body).await
}

/// The message AWS builds for one refused value: the count, the echoed value, the member
/// path and the member's own requirement.
fn pattern_refusal(value: &str, member: &str, expression: &str) -> String {
    format!(
        "1 validation error detected: Value '{value}' at '{member}' failed to satisfy \
         constraint: Member must satisfy regular expression pattern: {expression}"
    )
}

/// A `Resource` the pattern does not admit, and why it is outside it.
struct MalformedResource {
    name: &'static str,
    resource: String,
}

/// One row per way a value falls outside the shared pattern: not an ARN at all, an ARN of
/// another service, a resource type the alternation does not name, and — for each
/// alternative that carries a bounded id — a character outside its class, an empty id and
/// one past its published maximum.
fn malformed_resources() -> Vec<MalformedResource> {
    let too_long_provider = "p".repeat(MAX_PROVIDER_ID_LENGTH + 1);
    vec![
        MalformedResource {
            name: "not an ARN at all",
            resource: "totally-not-an-arn".to_string(),
        },
        MalformedResource {
            name: "a bare function name",
            resource: "my-function".to_string(),
        },
        MalformedResource {
            name: "another service's ARN",
            resource: format!("arn:aws:sqs:{REGION}:{ACCOUNT}:function:probe"),
        },
        MalformedResource {
            name: "a resource type the alternation does not name",
            resource: format!("arn:aws:lambda:{REGION}:{ACCOUNT}:alias:blue"),
        },
        MalformedResource {
            // The type segment is matched literally, so a different case is a different
            // segment.
            name: "the layer type segment in upper case",
            resource: format!("arn:aws:lambda:{REGION}:{ACCOUNT}:LAYER:my-layer"),
        },
        MalformedResource {
            // `layer:([a-zA-Z0-9-_]+)` admits no `.`.
            name: "a layer name carrying a character outside its class",
            resource: format!("arn:aws:lambda:{REGION}:{ACCOUNT}:layer:my.layer"),
        },
        MalformedResource {
            // `+` is one or more, so an empty layer name is outside the alternative.
            name: "an empty layer name",
            resource: format!("arn:aws:lambda:{REGION}:{ACCOUNT}:layer:"),
        },
        MalformedResource {
            // The account field is `\d{12}`; eleven digits is one short of it.
            name: "a layer ARN whose account id is a digit short",
            resource: format!("arn:aws:lambda:{REGION}:00000000000:layer:my-layer"),
        },
        MalformedResource {
            // `event-source-mapping:` carries a UUID, which this is not.
            name: "an event source mapping id that is no UUID",
            resource: format!("arn:aws:lambda:{REGION}:{ACCOUNT}:event-source-mapping:not-a-uuid"),
        },
        MalformedResource {
            // `csc-[a-z0-9]{17}` — sixteen characters is one short of it.
            name: "a code signing configuration id a character short",
            resource: format!("arn:aws:lambda:{REGION}:{ACCOUNT}:code-signing-config:csc-0123456789abcdef"),
        },
        MalformedResource {
            // `(capacity-provider|network-connector):[a-zA-Z0-9-_]{1,64}` — one past the
            // published maximum.
            name: "a capacity provider id one character past its maximum",
            resource: format!("arn:aws:lambda:{REGION}:{ACCOUNT}:capacity-provider:{too_long_provider}"),
        },
    ]
}

#[tokio::test]
async fn a_resource_outside_the_shared_pattern_is_refused_with_the_expression_stated_unanchored() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    assert!(
        !RESOURCE_EXPRESSION.starts_with('^') && !RESOURCE_EXPRESSION.ends_with('$'),
        "the stated expression carries no anchors, got {RESOURCE_EXPRESSION:?}"
    );

    for case in malformed_resources() {
        for operation in TAG_OPERATIONS {
            let answer = call_tag_operation(&router, operation, &case.resource).await;
            let case_name = format!("{} on {}", case.name, operation.name);
            assert_eq!(
                answer.status,
                400,
                "case {case_name:?} ({}): expected HTTP 400, got {} {}",
                case.resource,
                answer.status,
                answer.text()
            );
            let code = answer.error_code();
            assert_eq!(
                code, "ValidationException",
                "case {case_name:?} ({}): expected ValidationException, got {code:?} in {}",
                case.resource,
                answer.text()
            );
            let expected = pattern_refusal(&case.resource, "resource", RESOURCE_EXPRESSION);
            let message = answer.message();
            assert_eq!(
                message, expected,
                "case {case_name:?}: expected {expected:?}, got {message:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_pattern_requirement_of_another_member_is_stated_the_same_way() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    assert!(
        !ROLE_EXPRESSION.starts_with('^') && !ROLE_EXPRESSION.ends_with('$'),
        "the stated expression carries no anchors, got {ROLE_EXPRESSION:?}"
    );

    // One row per way a `Role` falls outside its own pattern: no ARN at all, an ARN of
    // another service, and an IAM ARN whose account field is a digit short.
    let rows = [
        ("not an ARN at all", "nobody"),
        (
            "another service's ARN",
            "arn:aws:lambda:us-east-1:000000000000:function:probe",
        ),
        (
            "an IAM role ARN whose account id is a digit short",
            "arn:aws:iam::00000000000:role/lambda-exec",
        ),
    ];
    for (case, role) in rows {
        let answer = create_function_with_role(&router, "role-refusals", role, &BTreeMap::new())
            .await;
        assert_eq!(
            answer.status,
            400,
            "case {case:?}: expected HTTP 400 from CreateFunction, got {} {}",
            answer.status,
            answer.text()
        );
        let expected = pattern_refusal(role, "role", ROLE_EXPRESSION);
        let message = answer.message();
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
    }
}

/// A `Resource` the pattern admits, and the answer `ListTags` — the one implemented tag
/// operation — gives for it out of state.
struct AdmittedResource {
    name: &'static str,
    resource: String,
    /// The status `ListTags` answers: `200` for a resource this account holds, `404` for
    /// a well-formed ARN naming a resource it does not.
    list_status: u16,
    /// The wire error code that answer carries, or `None` for the success.
    list_code: Option<&'static str>,
}

/// One row per alternative the pattern admits, varying the partition, the region and the
/// form of the resource id — including every shape of layer name the layer alternative
/// admits, which is the alternative this suite exists for.
fn admitted_resources(existing_function: &str) -> Vec<AdmittedResource> {
    let longest_provider = "p".repeat(MAX_PROVIDER_ID_LENGTH);
    vec![
        AdmittedResource {
            name: "a function this account holds",
            resource: format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{existing_function}"),
            list_status: 200,
            list_code: None,
        },
        AdmittedResource {
            name: "a function this account does not hold",
            resource: format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:no-such-function"),
            list_status: 404,
            list_code: Some("ResourceNotFoundException"),
        },
        AdmittedResource {
            name: "a layer",
            resource: format!("arn:aws:lambda:{REGION}:{ACCOUNT}:layer:my-layer"),
            list_status: 404,
            list_code: Some("ResourceNotFoundException"),
        },
        AdmittedResource {
            // `layer:([a-zA-Z0-9-_]+)` — every character class the alternative admits.
            name: "a layer whose name spans the admitted character class",
            resource: format!("arn:aws:lambda:{REGION}:{ACCOUNT}:layer:My_Layer-09"),
            list_status: 404,
            list_code: Some("ResourceNotFoundException"),
        },
        AdmittedResource {
            // One character is the shortest name the `+` admits.
            name: "a layer whose name is one character",
            resource: format!("arn:aws:lambda:{REGION}:{ACCOUNT}:layer:a"),
            list_status: 404,
            list_code: Some("ResourceNotFoundException"),
        },
        AdmittedResource {
            // The partition and region fields are their own alternations, so a layer in
            // another partition is admitted too.
            name: "a layer in the GovCloud partition",
            resource: format!("arn:aws-us-gov:lambda:us-gov-west-1:{ACCOUNT}:layer:gov-layer"),
            list_status: 404,
            list_code: Some("ResourceNotFoundException"),
        },
        AdmittedResource {
            name: "a code signing configuration",
            resource: format!(
                "arn:aws:lambda:{REGION}:{ACCOUNT}:code-signing-config:csc-0123456789abcdefg"
            ),
            list_status: 404,
            list_code: Some("ResourceNotFoundException"),
        },
        AdmittedResource {
            name: "an event source mapping",
            resource: format!(
                "arn:aws:lambda:{REGION}:{ACCOUNT}:event-source-mapping:\
                 12345678-1234-1234-1234-123456789012"
            ),
            list_status: 404,
            list_code: Some("ResourceNotFoundException"),
        },
        AdmittedResource {
            name: "a capacity provider",
            resource: format!("arn:aws:lambda:{REGION}:{ACCOUNT}:capacity-provider:fleet-1"),
            list_status: 404,
            list_code: Some("ResourceNotFoundException"),
        },
        AdmittedResource {
            name: "a capacity provider id at its published maximum",
            resource: format!("arn:aws:lambda:{REGION}:{ACCOUNT}:capacity-provider:{longest_provider}"),
            list_status: 404,
            list_code: Some("ResourceNotFoundException"),
        },
        AdmittedResource {
            name: "a network connector",
            resource: format!("arn:aws:lambda:{REGION}:{ACCOUNT}:network-connector:link-1"),
            list_status: 404,
            list_code: Some("ResourceNotFoundException"),
        },
    ]
}

#[tokio::test]
async fn every_resource_the_shared_pattern_admits_reaches_the_operation() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    // One function that really exists, so the success row below is answered out of state
    // rather than out of the refusal path.
    let tags: BTreeMap<String, String> = [("env".to_string(), "prod".to_string())]
        .into_iter()
        .collect();
    let created = create_function_with_role(&router, "tagged-function", ROLE, &tags).await;
    assert_eq!(
        created.status,
        201,
        "CreateFunction must succeed, got {} {}",
        created.status,
        created.text()
    );

    for case in admitted_resources("tagged-function") {
        for operation in TAG_OPERATIONS {
            let answer = call_tag_operation(&router, operation, &case.resource).await;
            let case_name = format!("{} on {}", case.name, operation.name);

            // The boundary accepted the value: whatever the operation answers, it is not
            // a constraint refusal naming the member's pattern.
            assert!(
                answer.error_code() != "ValidationException",
                "case {case_name:?} ({}): the pattern must admit this ARN, got {} {}",
                case.resource,
                answer.status,
                answer.text()
            );
            assert!(
                !answer.message().contains("regular expression pattern"),
                "case {case_name:?} ({}): expected no pattern requirement, got {}",
                case.resource,
                answer.text()
            );

            // `ListTags` is the implemented one, so its whole answer is pinned: the tag
            // set out of state for a resource this account holds, the modelled not-found
            // for a well-formed ARN naming one it does not.
            if operation.name != "ListTags" {
                continue;
            }
            assert_eq!(
                answer.status, case.list_status,
                "case {case_name:?} ({}): expected HTTP {}, got {} {}",
                case.resource,
                case.list_status,
                answer.status,
                answer.text()
            );
            match case.list_code {
                Some(code) => {
                    let actual = answer.error_code();
                    assert_eq!(
                        actual, code,
                        "case {case_name:?} ({}): expected error code {code:?}, got \
                         {actual:?} in {}",
                        case.resource,
                        answer.text()
                    );
                }
                None => {
                    let reported = answer.json().get("Tags").cloned();
                    let expected = json!({ "env": "prod" });
                    assert_eq!(
                        reported,
                        Some(expected.clone()),
                        "case {case_name:?}: expected Tags {expected}, got {reported:?}"
                    );
                }
            }
        }
    }
}
