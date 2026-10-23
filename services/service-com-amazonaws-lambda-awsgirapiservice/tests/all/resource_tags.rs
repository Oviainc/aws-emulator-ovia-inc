// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListTags`, the reporting half of Lambda's tag API.
//!
//! The rule under test: a Lambda tag set belongs to the resource its ARN names, so
//! `ListTags` answers the tag set that resource currently carries and nothing else — a
//! resource carrying no tags answers an empty tag set, an ARN that qualifies a function
//! with a version or an alias names no tag set at all (`Resource`'s own documentation:
//! "Lambda does not support adding tags to function aliases or versions"), and an ARN
//! naming a resource this account does not hold is refused as not found. Because the
//! operation only reports, two reads in a row answer the same thing.
//!
//! Every row drives real HTTP requests through the router: the tags go in through
//! `CreateFunction` ("A list of tags to apply to the function"), come back out through
//! `ListTags`, and are cross-checked against the `Tags` of `GetFunction`, which the
//! model points at for the same data ("You can also view function tags with
//! GetFunction"). A tag set that was accepted but never stored, or reported from
//! anywhere other than the stored record, fails here.
//!
//! The rows differ along what the rule generalises over: how MANY tags the resource
//! carries (none, one, several), the FORM of the keys and values (empty value, the whole
//! character class the `TagKey`/`TagValue` pattern admits, non-ASCII letters, both
//! published length bounds), and — for the refusals — the resource TYPE the ARN names
//! and whether it carries a qualifier.

use std::collections::BTreeMap;
use std::sync::Arc;

use base64::Engine as _;
use serde_json::{json, Map, Value};
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// Where `CreateFunction` and `GetFunction` are bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// Where the tag operations are bound (`smithy_query(query="operation",
/// operation="ListTags")`: `GET /2017-03-31/tags/{Resource}`).
const TAGS_URI: &str = "/2017-03-31/tags";

/// A SigV4 credential scope naming this service's signing name (`lambda`) in
/// `us-east-1`, which is what routes a request here and what region the ARNs carry.
/// Nothing in it is verified: the harness turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/lambda/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// The region the scope above names.
const REGION: &str = "us-east-1";

/// The account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The longest `TagKey` the model admits (`length` 1..128).
const MAX_TAG_KEY_LENGTH: usize = 128;

/// The longest `TagValue` the model admits (`length` 0..256).
const MAX_TAG_VALUE_LENGTH: usize = 256;

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

    /// The wire error code (`__type`, or the `x-amzn-errortype` header).
    fn error_code(&self) -> String {
        self.json()
            .get("__type")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                self.headers
                    .get("x-amzn-errortype")
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| String::from_utf8_lossy(&self.body).to_string())
            // An error code may arrive namespaced (`com.amazonaws…#Code`) or with a
            // trailing URL; a client matches on the bare shape name.
            .rsplit(['#', '/'])
            .next()
            .unwrap_or_default()
            .to_string()
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

/// The unqualified ARN a function of `name` carries.
fn function_arn(name: &str) -> String {
    format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{name}")
}

/// `CreateFunction`, with the tag set the row describes.
async fn create_function(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    tags: &BTreeMap<String, String>,
    publish: bool,
) {
    let package: Vec<u8> = (0..64u16)
        .map(|byte| u8::try_from(byte % 251).unwrap_or(0))
        .collect();
    let mut body = json!({
        "FunctionName": name,
        "Role": ROLE,
        "Runtime": "python3.12",
        "Handler": "index.handler",
        "Publish": publish,
        "Code": { "ZipFile": base64::engine::general_purpose::STANDARD.encode(&package) },
    });
    if !tags.is_empty() {
        body["Tags"] = Value::Object(
            tags.iter()
                .map(|(key, value)| (key.clone(), Value::String(value.clone())))
                .collect::<Map<String, Value>>(),
        );
    }
    let answer = call(router, "POST", FUNCTIONS_URI, &body).await;
    assert_eq!(
        answer.status,
        201,
        "case {case:?}: CreateFunction must succeed, got {} {}",
        answer.status,
        answer.text()
    );
}

/// `ListTags` on `resource`.
async fn list_tags(router: &MultiServiceRouter, resource: &str) -> Answer {
    call(router, "GET", &format!("{TAGS_URI}/{resource}"), &Value::Null).await
}

/// `GetFunction` on `name`.
async fn get_function(router: &MultiServiceRouter, name: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("{FUNCTIONS_URI}/{name}"),
        &Value::Null,
    )
    .await
}

/// The `Tags` member of a successful answer, as an ordered map. Panics when the member
/// is absent or is not a map of strings, which is the failure a caller needs to see.
fn tags_of(case: &str, answer: &Answer) -> BTreeMap<String, String> {
    let body = answer.json();
    let tags = body
        .get("Tags")
        .unwrap_or_else(|| panic!("case {case:?}: no Tags member in {}", answer.text()));
    tags.as_object()
        .unwrap_or_else(|| panic!("case {case:?}: Tags is not a map: {tags}"))
        .iter()
        .map(|(key, value)| {
            let text = value.as_str().unwrap_or_else(|| {
                panic!("case {case:?}: tag {key:?} is not a string: {value}")
            });
            (key.clone(), text.to_string())
        })
        .collect()
}

/// A tag set from `(key, value)` pairs.
fn tag_set(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// One row of the tag-set table: the function to create, the tags it is created with,
/// and whether a version is published at the same time.
struct TagCase {
    name: &'static str,
    function: &'static str,
    publish: bool,
    tags: BTreeMap<String, String>,
}

/// The rows, each differing from the others in how many tags the function carries or in
/// the form of its keys and values — the dimensions the rule generalises over.
fn tag_cases() -> Vec<TagCase> {
    vec![
        TagCase {
            name: "a function created with no tags carries an empty tag set",
            function: "untagged",
            publish: false,
            tags: BTreeMap::new(),
        },
        TagCase {
            name: "one tag",
            function: "one-tag",
            publish: false,
            tags: tag_set(&[("env", "prod")]),
        },
        TagCase {
            name: "several tags",
            function: "many-tags",
            publish: false,
            tags: tag_set(&[
                ("env", "staging"),
                ("owner", "platform"),
                ("cost-centre", "4815"),
            ]),
        },
        TagCase {
            // `TagValue` has `length` min 0, so the empty string is a value a tag may
            // hold and must be reported as one rather than dropped.
            name: "a tag whose value is empty",
            function: "empty-value",
            publish: false,
            tags: tag_set(&[("reviewed", "")]),
        },
        TagCase {
            // Every separator and sign the `TagKey`/`TagValue` pattern
            // `^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$` admits, including a `\p{Z}` space.
            name: "keys and values spanning the admitted character class",
            function: "punctuated-tags",
            publish: false,
            tags: tag_set(&[
                ("cost.center/unit:1", "a_b-c+d=e@f.g/h 2"),
                ("team name", "core platform"),
            ]),
        },
        TagCase {
            // `\p{L}` and `\p{N}` are Unicode categories, so a non-ASCII letter is an
            // admitted tag character.
            name: "non-ASCII letters in both key and value",
            function: "unicode-tags",
            publish: false,
            tags: tag_set(&[("équipe", "paiement-intégré"), ("ключ", "значение")]),
        },
        TagCase {
            // Both published bounds at once: `TagKey` 1..128 and `TagValue` 0..256.
            name: "a key and a value at their maximum lengths",
            function: "bounded-tags",
            publish: false,
            tags: [(
                "k".repeat(MAX_TAG_KEY_LENGTH),
                "v".repeat(MAX_TAG_VALUE_LENGTH),
            )]
            .into_iter()
            .collect(),
        },
        TagCase {
            // Tags are function-level, so publishing a version at create time changes
            // nothing about the set the function's own ARN reports.
            name: "a function published at create time",
            function: "published-tags",
            publish: true,
            tags: tag_set(&[("tier", "gold")]),
        },
    ]
}

#[tokio::test]
async fn list_tags_reports_the_tag_set_the_named_function_holds() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    for case in tag_cases() {
        let TagCase {
            name,
            function,
            publish,
            tags,
        } = case;
        create_function(&router, name, function, &tags, publish).await;

        let arn = function_arn(function);
        let answer = list_tags(&router, &arn).await;
        assert_eq!(
            answer.status,
            200,
            "case {name:?}: ListTags must answer 200, got {} {}",
            answer.status,
            answer.text()
        );
        let reported = tags_of(name, &answer);
        assert_eq!(
            reported, tags,
            "case {name:?}: expected tags {tags:?}, got {reported:?}"
        );

        // The same data through the operation the model points at for it. `GetFunction`
        // omits `Tags` entirely for a function carrying none, so an empty set is checked
        // as an absent member there.
        let configured = get_function(&router, function).await;
        assert_eq!(
            configured.status,
            200,
            "case {name:?}: GetFunction must answer 200, got {} {}",
            configured.status,
            configured.text()
        );
        if tags.is_empty() {
            let member = configured.json().get("Tags").cloned();
            assert!(
                member.is_none(),
                "case {name:?}: GetFunction must omit Tags for an untagged function, got \
                 {member:?}"
            );
        } else {
            let from_get = tags_of(name, &configured);
            assert_eq!(
                from_get, tags,
                "case {name:?}: GetFunction must report the same tags {tags:?}, got {from_get:?}"
            );
        }

        // Reporting changes nothing: a second read answers the same set.
        let again = list_tags(&router, &arn).await;
        let repeated = tags_of(name, &again);
        assert_eq!(
            repeated, tags,
            "case {name:?}: a second ListTags must answer the same tags {tags:?}, got \
             {repeated:?}"
        );
    }
}

/// One row of the refusal table: the `Resource` to ask for, and the status and wire
/// error code the rule requires for it.
struct RefusalCase {
    name: &'static str,
    resource: String,
    status: u16,
    code: &'static str,
}

/// The refusals, one per way a `Resource` can name no tag set: a function this account
/// does not have, a qualifier (version or alias) in any spelling, each other resource
/// type the `TaggableResource` pattern admits but this account holds none of, and a
/// value that is not a resource ARN at all.
fn refusal_cases() -> Vec<RefusalCase> {
    vec![
        RefusalCase {
            name: "a function this account does not have",
            resource: function_arn("no-such-function"),
            status: 404,
            code: "ResourceNotFoundException",
        },
        RefusalCase {
            name: "a published version of a function that exists",
            resource: format!("{}:1", function_arn("published-tags")),
            status: 400,
            code: "InvalidParameterValueException",
        },
        RefusalCase {
            name: "the unpublished version label",
            resource: format!("{}:$LATEST", function_arn("published-tags")),
            status: 400,
            code: "InvalidParameterValueException",
        },
        RefusalCase {
            name: "an alias of a function that exists",
            resource: format!("{}:blue", function_arn("published-tags")),
            status: 400,
            code: "InvalidParameterValueException",
        },
        RefusalCase {
            // `code-signing-config:csc-[a-z0-9]{17}`.
            name: "a code signing configuration",
            resource: format!(
                "arn:aws:lambda:{REGION}:{ACCOUNT}:code-signing-config:csc-0123456789abcdefg"
            ),
            status: 404,
            code: "ResourceNotFoundException",
        },
        RefusalCase {
            // `event-source-mapping:<uuid>`.
            name: "an event source mapping",
            resource: format!(
                "arn:aws:lambda:{REGION}:{ACCOUNT}:event-source-mapping:\
                 12345678-1234-1234-1234-123456789012"
            ),
            status: 404,
            code: "ResourceNotFoundException",
        },
        RefusalCase {
            name: "a capacity provider",
            resource: format!("arn:aws:lambda:{REGION}:{ACCOUNT}:capacity-provider:fleet-1"),
            status: 404,
            code: "ResourceNotFoundException",
        },
        RefusalCase {
            name: "a network connector",
            resource: format!("arn:aws:lambda:{REGION}:{ACCOUNT}:network-connector:link-1"),
            status: 404,
            code: "ResourceNotFoundException",
        },
        RefusalCase {
            // The `Resource` member is an ARN, never a bare function name, so a name is
            // outside the member's own constraint and is refused at the boundary.
            name: "a bare function name",
            resource: "published-tags".to_string(),
            status: 400,
            code: "ValidationException",
        },
    ]
}

#[tokio::test]
async fn list_tags_refuses_a_resource_that_names_no_tag_set() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    // One function that really exists, so a refusal below is about the ARN's shape or
    // the resource type rather than about an absent function.
    create_function(
        &router,
        "a function to qualify",
        "published-tags",
        &tag_set(&[("tier", "gold")]),
        true,
    )
    .await;

    for case in refusal_cases() {
        let answer = list_tags(&router, &case.resource).await;
        assert_eq!(
            answer.status, case.status,
            "case {:?} ({}): expected HTTP {}, got {} {}",
            case.name,
            case.resource,
            case.status,
            answer.status,
            answer.text()
        );
        let code = answer.error_code();
        assert_eq!(
            code, case.code,
            "case {:?} ({}): expected error code {:?}, got {code:?} in {}",
            case.name,
            case.resource,
            case.code,
            answer.text()
        );
    }

    // The function the qualified rows name is untouched by those refusals: its own ARN
    // still reports the tag set it was created with.
    let answer = list_tags(&router, &function_arn("published-tags")).await;
    let reported = tags_of("after the refusals", &answer);
    let expected = tag_set(&[("tier", "gold")]);
    assert_eq!(
        reported, expected,
        "after the refusals: expected tags {expected:?}, got {reported:?}"
    );
}
