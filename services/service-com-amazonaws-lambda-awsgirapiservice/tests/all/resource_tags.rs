// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for Lambda's tag API: `ListTags`, which reports a resource's tag
//! set, and `TagResource`/`UntagResource`, which write it.
//!
//! The rule under test: a Lambda tag set belongs to the resource its ARN names, so
//! `ListTags` answers the tag set that resource currently carries and nothing else — a
//! resource carrying no tags answers an empty tag set, an ARN that qualifies a function
//! with a version or an alias names no tag set at all (`Resource`'s own documentation:
//! "Lambda does not support adding tags to function aliases or versions"), and an ARN
//! naming a resource this account does not hold is refused as not found. Because the
//! operation only reports, two reads in a row answer the same thing.
//!
//! `TagResource` MERGES into that set — a key the request names takes the value the
//! request gives it, replacing whatever it held, and a key the request does not name is
//! left as it was — and `UntagResource` removes exactly the keys it lists, a key the
//! resource does not carry being a no-op rather than a refusal. Both name the same
//! resource space as `ListTags`, and both refuse the same way when the ARN names no tag
//! set. A resource carries at most 50 tags, so a write that would leave it holding more
//! is refused whole.
//!
//! Every row drives real HTTP requests through the router: the tags go in through
//! `CreateFunction` ("A list of tags to apply to the function") or `TagResource`, come
//! back out through `ListTags`, and are cross-checked against the `Tags` of
//! `GetFunction`, which the model points at for the same data ("You can also view
//! function tags with GetFunction"). A tag set that was accepted but never stored, or
//! reported from anywhere other than the stored record, fails here.
//!
//! The rows differ along what the rule generalises over: how MANY tags the resource
//! carries (none, one, several, up to and past the published limit), how many CALLS the
//! set is built up or torn down over, whether a written key is new or already present,
//! the FORM of the keys and values (empty value, the whole character class the
//! `TagKey`/`TagValue` pattern admits, non-ASCII letters, both published length bounds),
//! and — for the refusals — the resource TYPE the ARN names and whether it carries a
//! qualifier.

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

/// `TagResource` on `resource`, with the tags to merge in.
async fn tag_resource(
    router: &MultiServiceRouter,
    resource: &str,
    tags: &BTreeMap<String, String>,
) -> Answer {
    let body = json!({
        "Tags": Value::Object(
            tags.iter()
                .map(|(key, value)| (key.clone(), Value::String(value.clone())))
                .collect::<Map<String, Value>>(),
        ),
    });
    call(router, "POST", &format!("{TAGS_URI}/{resource}"), &body).await
}

/// `UntagResource` on `resource`, with the keys to remove. `TagKeys` is bound to the
/// `tagKeys` query parameter (`smithy_query(query="operation",
/// operation="UntagResource")`), so each key is one repetition of it.
async fn untag_resource(router: &MultiServiceRouter, resource: &str, keys: &[&str]) -> Answer {
    let query = keys
        .iter()
        .map(|key| format!("tagKeys={}", query_encoded(key)))
        .collect::<Vec<String>>()
        .join("&");
    call(
        router,
        "DELETE",
        &format!("{TAGS_URI}/{resource}?{query}"),
        &Value::Null,
    )
    .await
}

/// One query-parameter value, percent-encoded outside RFC 3986's unreserved set, so a
/// key carrying a space or a non-ASCII letter survives the URI.
fn query_encoded(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (byte as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
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

/// The most tags one Lambda resource may carry. AWS publishes it as a tagging
/// restriction rather than in the model: the Lambda Developer Guide's "Tag Lambda
/// resources" states "Maximum number of tags per resource: 50".
const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The status `TagResource` and `UntagResource` answer on success (`smithy_query(
/// query="operation", operation="TagResource")`: `POST /2017-03-31/tags/{Resource}`,
/// code 204).
const TAG_WRITE_STATUS: u16 = 204;

/// `count` distinct tags, keyed `<prefix>-00`, `<prefix>-01`, …, so a row can ask for a
/// tag set of any size without spelling every pair out.
fn numbered_tags(prefix: &str, count: usize) -> BTreeMap<String, String> {
    (0..count)
        .map(|index| (format!("{prefix}-{index:02}"), format!("value-{index:02}")))
        .collect()
}

/// One row of the merge table: the tags the function is created with, the `TagResource`
/// calls to make in order, and the tag set the resource must hold afterwards.
struct MergeCase {
    name: &'static str,
    function: &'static str,
    created_with: BTreeMap<String, String>,
    writes: Vec<BTreeMap<String, String>>,
    expected: BTreeMap<String, String>,
}

/// The rows, each differing from the others in how many keys one call writes, how many
/// CALLS build the set up, whether a written key is new or already present, or the form
/// of the keys and values — the dimensions the merge rule generalises over.
fn merge_cases() -> Vec<MergeCase> {
    vec![
        MergeCase {
            name: "a first tag on a function created without any",
            function: "merge-first",
            created_with: BTreeMap::new(),
            writes: vec![tag_set(&[("env", "prod")])],
            expected: tag_set(&[("env", "prod")]),
        },
        MergeCase {
            name: "several keys in one call",
            function: "merge-several",
            created_with: BTreeMap::new(),
            writes: vec![tag_set(&[
                ("env", "staging"),
                ("owner", "platform"),
                ("cost-centre", "4815"),
            ])],
            expected: tag_set(&[
                ("env", "staging"),
                ("owner", "platform"),
                ("cost-centre", "4815"),
            ]),
        },
        MergeCase {
            name: "a new key keeps the one the function was created with",
            function: "merge-keeps",
            created_with: tag_set(&[("env", "prod")]),
            writes: vec![tag_set(&[("owner", "platform")])],
            expected: tag_set(&[("env", "prod"), ("owner", "platform")]),
        },
        MergeCase {
            name: "writing a key that is already set replaces its value",
            function: "merge-overwrite",
            created_with: tag_set(&[("env", "prod")]),
            writes: vec![tag_set(&[("env", "staging")])],
            expected: tag_set(&[("env", "staging")]),
        },
        MergeCase {
            name: "one call that replaces one key and adds another",
            function: "merge-mixed",
            created_with: tag_set(&[("env", "prod"), ("owner", "platform")]),
            writes: vec![tag_set(&[("owner", "payments"), ("tier", "gold")])],
            expected: tag_set(&[("env", "prod"), ("owner", "payments"), ("tier", "gold")]),
        },
        MergeCase {
            name: "the same call twice leaves the same set",
            function: "merge-idempotent",
            created_with: tag_set(&[("env", "prod")]),
            writes: vec![tag_set(&[("env", "prod")]), tag_set(&[("env", "prod")])],
            expected: tag_set(&[("env", "prod")]),
        },
        MergeCase {
            name: "four calls in a row accumulate, one key each",
            function: "merge-accumulates",
            created_with: tag_set(&[("a", "1")]),
            writes: vec![
                tag_set(&[("b", "2")]),
                tag_set(&[("c", "3")]),
                tag_set(&[("d", "4")]),
                tag_set(&[("e", "5")]),
            ],
            expected: tag_set(&[("a", "1"), ("b", "2"), ("c", "3"), ("d", "4"), ("e", "5")]),
        },
        MergeCase {
            // `TagValue` has `length` min 0, so the empty string is a value a key may be
            // overwritten with, and the key stays rather than disappearing.
            name: "a value replaced by the empty string",
            function: "merge-empty-value",
            created_with: tag_set(&[("reviewed", "yes")]),
            writes: vec![tag_set(&[("reviewed", "")])],
            expected: tag_set(&[("reviewed", "")]),
        },
        MergeCase {
            // Every separator and sign the `TagKey`/`TagValue` pattern
            // `^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$` admits, including a `\p{Z}` space.
            name: "keys and values spanning the admitted character class",
            function: "merge-punctuated",
            created_with: BTreeMap::new(),
            writes: vec![tag_set(&[
                ("cost.center/unit:1", "a_b-c+d=e@f.g/h 2"),
                ("team name", "core platform"),
            ])],
            expected: tag_set(&[
                ("cost.center/unit:1", "a_b-c+d=e@f.g/h 2"),
                ("team name", "core platform"),
            ]),
        },
        MergeCase {
            // `\p{L}` and `\p{N}` are Unicode categories, so a non-ASCII letter is an
            // admitted tag character.
            name: "non-ASCII letters in both key and value",
            function: "merge-unicode",
            created_with: BTreeMap::new(),
            writes: vec![tag_set(&[("équipe", "paiement-intégré"), ("ключ", "значение")])],
            expected: tag_set(&[("équipe", "paiement-intégré"), ("ключ", "значение")]),
        },
        MergeCase {
            // Both published bounds at once: `TagKey` 1..128 and `TagValue` 0..256.
            name: "a key and a value at their maximum lengths",
            function: "merge-bounded",
            created_with: BTreeMap::new(),
            writes: vec![[(
                "k".repeat(MAX_TAG_KEY_LENGTH),
                "v".repeat(MAX_TAG_VALUE_LENGTH),
            )]
            .into_iter()
            .collect()],
            expected: [(
                "k".repeat(MAX_TAG_KEY_LENGTH),
                "v".repeat(MAX_TAG_VALUE_LENGTH),
            )]
            .into_iter()
            .collect(),
        },
    ]
}

#[tokio::test]
async fn tag_resource_merges_its_tags_into_the_set_the_resource_holds() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    for case in merge_cases() {
        let MergeCase {
            name,
            function,
            created_with,
            writes,
            expected,
        } = case;
        create_function(&router, name, function, &created_with, false).await;
        let arn = function_arn(function);

        for (index, write) in writes.iter().enumerate() {
            let answer = tag_resource(&router, &arn, write).await;
            assert_eq!(
                answer.status,
                TAG_WRITE_STATUS,
                "case {name:?}: TagResource call {index} must answer \
                 {TAG_WRITE_STATUS}, got {} {}",
                answer.status,
                answer.text()
            );
            assert!(
                answer.body.is_empty(),
                "case {name:?}: TagResource call {index} answers no body, got {}",
                answer.text()
            );
        }

        // Read the set back through the operation that reports it...
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
            reported, expected,
            "case {name:?}: expected tags {expected:?}, got {reported:?}"
        );

        // ...and through the one the model points at for the same data, so the write
        // landed on the function's own record rather than beside it.
        let configured = get_function(&router, function).await;
        let from_get = tags_of(name, &configured);
        assert_eq!(
            from_get, expected,
            "case {name:?}: GetFunction must report the same tags {expected:?}, got {from_get:?}"
        );
    }
}

/// One row of the untag table: the tags the function is created with, the
/// `UntagResource` calls to make in order, and the tag set left afterwards.
struct UntagCase {
    name: &'static str,
    function: &'static str,
    created_with: BTreeMap<String, String>,
    removals: Vec<Vec<String>>,
    expected: BTreeMap<String, String>,
}

/// The keys of one `UntagResource` call.
fn keys(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_string()).collect()
}

/// The rows, each differing in how many keys one call removes, how many calls do the
/// removing, whether a listed key is present, and the form of the keys.
fn untag_cases() -> Vec<UntagCase> {
    vec![
        UntagCase {
            name: "one key of several",
            function: "untag-one",
            created_with: tag_set(&[("env", "prod"), ("owner", "platform")]),
            removals: vec![keys(&["env"])],
            expected: tag_set(&[("owner", "platform")]),
        },
        UntagCase {
            name: "several keys in one call",
            function: "untag-several",
            created_with: tag_set(&[("a", "1"), ("b", "2"), ("c", "3"), ("d", "4")]),
            removals: vec![keys(&["a", "c"])],
            expected: tag_set(&[("b", "2"), ("d", "4")]),
        },
        UntagCase {
            // The no-op: nothing is held under that key, so there is nothing to remove
            // and the rest of the set is untouched.
            name: "a key the resource never carried",
            function: "untag-unset",
            created_with: tag_set(&[("env", "prod")]),
            removals: vec![keys(&["owner"])],
            expected: tag_set(&[("env", "prod")]),
        },
        UntagCase {
            name: "one call listing a key that is set and one that is not",
            function: "untag-mixed",
            created_with: tag_set(&[("env", "prod"), ("owner", "platform")]),
            removals: vec![keys(&["env", "never-set"])],
            expected: tag_set(&[("owner", "platform")]),
        },
        UntagCase {
            name: "the same key twice, the second call a no-op",
            function: "untag-twice",
            created_with: tag_set(&[("env", "prod"), ("owner", "platform")]),
            removals: vec![keys(&["env"]), keys(&["env"])],
            expected: tag_set(&[("owner", "platform")]),
        },
        UntagCase {
            name: "every key the resource carries, over two calls",
            function: "untag-all",
            created_with: tag_set(&[("a", "1"), ("b", "2"), ("c", "3")]),
            removals: vec![keys(&["a", "b"]), keys(&["c"])],
            expected: BTreeMap::new(),
        },
        UntagCase {
            // A key spanning the admitted character class, including a `\p{Z}` space and
            // the separators, so the query parameter carries it percent-encoded.
            name: "keys carrying separators and a space",
            function: "untag-punctuated",
            created_with: tag_set(&[
                ("cost.center/unit:1", "4815"),
                ("team name", "core platform"),
                ("kept", "yes"),
            ]),
            removals: vec![keys(&["cost.center/unit:1", "team name"])],
            expected: tag_set(&[("kept", "yes")]),
        },
        UntagCase {
            name: "a key of non-ASCII letters",
            function: "untag-unicode",
            created_with: tag_set(&[("équipe", "paiement"), ("ключ", "значение")]),
            removals: vec![keys(&["équipe"])],
            expected: tag_set(&[("ключ", "значение")]),
        },
        UntagCase {
            // `TagKey` has `length` max 128, so a key at that bound is one a resource can
            // carry and therefore one `UntagResource` has to be able to name.
            name: "a key at its maximum length",
            function: "untag-bounded",
            created_with: [
                ("k".repeat(MAX_TAG_KEY_LENGTH), "v".to_string()),
                ("kept".to_string(), "yes".to_string()),
            ]
            .into_iter()
            .collect(),
            removals: vec![vec!["k".repeat(MAX_TAG_KEY_LENGTH)]],
            expected: tag_set(&[("kept", "yes")]),
        },
    ]
}

#[tokio::test]
async fn untag_resource_removes_only_the_keys_it_lists() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    for case in untag_cases() {
        let UntagCase {
            name,
            function,
            created_with,
            removals,
            expected,
        } = case;
        create_function(&router, name, function, &created_with, false).await;
        let arn = function_arn(function);

        for (index, removal) in removals.iter().enumerate() {
            let listed: Vec<&str> = removal.iter().map(String::as_str).collect();
            let answer = untag_resource(&router, &arn, &listed).await;
            assert_eq!(
                answer.status,
                TAG_WRITE_STATUS,
                "case {name:?}: UntagResource call {index} ({listed:?}) must answer \
                 {TAG_WRITE_STATUS}, got {} {}",
                answer.status,
                answer.text()
            );
            assert!(
                answer.body.is_empty(),
                "case {name:?}: UntagResource call {index} answers no body, got {}",
                answer.text()
            );
        }

        let answer = list_tags(&router, &arn).await;
        let reported = tags_of(name, &answer);
        assert_eq!(
            reported, expected,
            "case {name:?}: expected tags {expected:?}, got {reported:?}"
        );
    }
}

/// One row of the limit table: how many tags the function is created with, the tags one
/// `TagResource` call then writes, and whether that call is accepted.
struct LimitCase {
    name: &'static str,
    function: &'static str,
    created_with: BTreeMap<String, String>,
    written: BTreeMap<String, String>,
    accepted: bool,
}

/// The rows cover each side of the published bound: one tag short of it, exactly it, one
/// past it, the bound reached across two calls, one past it across two calls, and a write
/// at the bound that only REPLACES a key (so the set does not grow and is accepted).
fn limit_cases() -> Vec<LimitCase> {
    let at_limit = numbered_tags("held", MAX_TAGS_PER_RESOURCE);
    let replacement: BTreeMap<String, String> = at_limit
        .keys()
        .take(1)
        .map(|key| (key.clone(), "replaced".to_string()))
        .collect();
    vec![
        LimitCase {
            name: "one tag short of the limit in a single call",
            function: "limit-short",
            created_with: BTreeMap::new(),
            written: numbered_tags("add", MAX_TAGS_PER_RESOURCE - 1),
            accepted: true,
        },
        LimitCase {
            name: "exactly the limit in a single call",
            function: "limit-exact",
            created_with: BTreeMap::new(),
            written: numbered_tags("add", MAX_TAGS_PER_RESOURCE),
            accepted: true,
        },
        LimitCase {
            name: "one past the limit in a single call",
            function: "limit-past",
            created_with: BTreeMap::new(),
            written: numbered_tags("add", MAX_TAGS_PER_RESOURCE + 1),
            accepted: false,
        },
        LimitCase {
            name: "the limit reached across the create and one call",
            function: "limit-split",
            created_with: numbered_tags("held", 20),
            written: numbered_tags("add", MAX_TAGS_PER_RESOURCE - 20),
            accepted: true,
        },
        LimitCase {
            name: "one key past the limit, at the limit already",
            function: "limit-full",
            created_with: at_limit.clone(),
            written: numbered_tags("add", 1),
            accepted: false,
        },
        LimitCase {
            name: "replacing a key at the limit does not grow the set",
            function: "limit-replace",
            created_with: at_limit,
            written: replacement,
            accepted: true,
        },
    ]
}

#[tokio::test]
async fn tag_resource_refuses_a_write_that_would_pass_the_tag_limit() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    for case in limit_cases() {
        let LimitCase {
            name,
            function,
            created_with,
            written,
            accepted,
        } = case;
        create_function(&router, name, function, &created_with, false).await;
        let arn = function_arn(function);

        let answer = tag_resource(&router, &arn, &written).await;
        // An accepted write leaves the merged set; a refused one leaves the set the
        // resource already had, whole — never the prefix of the request that fitted.
        let expected: BTreeMap<String, String> = if accepted {
            let mut merged = created_with.clone();
            merged.extend(written.clone());
            merged
        } else {
            created_with.clone()
        };

        if accepted {
            assert_eq!(
                answer.status,
                TAG_WRITE_STATUS,
                "case {name:?}: {} tags must be accepted, got {} {}",
                expected.len(),
                answer.status,
                answer.text()
            );
        } else {
            assert_eq!(
                answer.status,
                400,
                "case {name:?}: a write past {MAX_TAGS_PER_RESOURCE} tags must answer 400, \
                 got {} {}",
                answer.status,
                answer.text()
            );
            let code = answer.error_code();
            assert_eq!(
                code, "InvalidParameterValueException",
                "case {name:?}: expected error code \"InvalidParameterValueException\", got \
                 {code:?} in {}",
                answer.text()
            );
        }

        let reported = tags_of(name, &list_tags(&router, &arn).await);
        assert_eq!(
            reported.len(),
            expected.len(),
            "case {name:?}: expected {} tags held, got {}",
            expected.len(),
            reported.len()
        );
        assert_eq!(
            reported, expected,
            "case {name:?}: expected tags {expected:?}, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn concurrent_tag_writes_on_one_resource_all_survive() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    create_function(
        &router,
        "concurrent writers",
        "concurrent-tags",
        &tag_set(&[("created", "yes")]),
        false,
    )
    .await;
    let arn = function_arn("concurrent-tags");

    // Four callers tagging the same function at the same time, each with its own key.
    // Merging (rather than replacing the set) is what makes every one of them survive.
    let writes = [
        tag_set(&[("writer", "one")]),
        tag_set(&[("owner", "two")]),
        tag_set(&[("tier", "three")]),
        tag_set(&[("env", "four")]),
    ];
    let (first, second, third, fourth) = tokio::join!(
        tag_resource(&router, &arn, &writes[0]),
        tag_resource(&router, &arn, &writes[1]),
        tag_resource(&router, &arn, &writes[2]),
        tag_resource(&router, &arn, &writes[3]),
    );
    for (index, answer) in [first, second, third, fourth].iter().enumerate() {
        assert_eq!(
            answer.status,
            TAG_WRITE_STATUS,
            "concurrent writer {index}: expected {TAG_WRITE_STATUS}, got {} {}",
            answer.status,
            answer.text()
        );
    }

    let expected = tag_set(&[
        ("created", "yes"),
        ("writer", "one"),
        ("owner", "two"),
        ("tier", "three"),
        ("env", "four"),
    ]);
    let reported = tags_of("concurrent writers", &list_tags(&router, &arn).await);
    assert_eq!(
        reported, expected,
        "concurrent writers: expected tags {expected:?}, got {reported:?}"
    );
}

/// Where `CreateCapacityProvider` is bound (`smithy_query(query="operation",
/// operation="CreateCapacityProvider")`: `POST /2025-11-30/capacity-providers`).
const CAPACITY_PROVIDERS_URI: &str = "/2025-11-30/capacity-providers";

/// A well-formed operator role ARN, satisfying the model's `RoleArn` pattern.
const OPERATOR_ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-capacity";

#[tokio::test]
async fn tag_writes_reach_every_resource_type_whose_record_holds_tags() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    // A capacity provider is the other resource this store holds a tag set for, so the
    // same two operations have to write ITS record too, not only a function's.
    let created = call(
        &router,
        "POST",
        CAPACITY_PROVIDERS_URI,
        &json!({
            "CapacityProviderName": "tagged-fleet",
            "VpcConfig": {
                "SubnetIds": ["subnet-10000000"],
                "SecurityGroupIds": [],
            },
            "PermissionsConfig": { "CapacityProviderOperatorRoleArn": OPERATOR_ROLE },
            "Tags": { "created": "yes" },
        }),
    )
    .await;
    assert_eq!(
        created.status,
        202,
        "CreateCapacityProvider must succeed, got {} {}",
        created.status,
        created.text()
    );
    let provider = format!("arn:aws:lambda:{REGION}:{ACCOUNT}:capacity-provider:tagged-fleet");

    let answer = tag_resource(&router, &provider, &tag_set(&[("env", "prod")])).await;
    assert_eq!(
        answer.status,
        TAG_WRITE_STATUS,
        "TagResource on a capacity provider must answer {TAG_WRITE_STATUS}, got {} {}",
        answer.status,
        answer.text()
    );
    let expected = tag_set(&[("created", "yes"), ("env", "prod")]);
    let reported = tags_of("a tagged capacity provider", &list_tags(&router, &provider).await);
    assert_eq!(
        reported, expected,
        "a tagged capacity provider: expected tags {expected:?}, got {reported:?}"
    );

    let answer = untag_resource(&router, &provider, &["created"]).await;
    assert_eq!(
        answer.status,
        TAG_WRITE_STATUS,
        "UntagResource on a capacity provider must answer {TAG_WRITE_STATUS}, got {} {}",
        answer.status,
        answer.text()
    );
    let expected = tag_set(&[("env", "prod")]);
    let reported = tags_of("an untagged capacity provider", &list_tags(&router, &provider).await);
    assert_eq!(
        reported, expected,
        "an untagged capacity provider: expected tags {expected:?}, got {reported:?}"
    );
}

#[tokio::test]
async fn tag_and_untag_refuse_a_resource_that_names_no_tag_set() {
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

    // A write names the same resource space a read does, so the refusals are the same
    // rows — each checked for both of the writing operations.
    for case in refusal_cases() {
        let tagged = tag_resource(&router, &case.resource, &tag_set(&[("env", "prod")])).await;
        assert_eq!(
            tagged.status, case.status,
            "case {:?} ({}): TagResource expected HTTP {}, got {} {}",
            case.name,
            case.resource,
            case.status,
            tagged.status,
            tagged.text()
        );
        let code = tagged.error_code();
        assert_eq!(
            code, case.code,
            "case {:?} ({}): TagResource expected error code {:?}, got {code:?} in {}",
            case.name,
            case.resource,
            case.code,
            tagged.text()
        );

        let untagged = untag_resource(&router, &case.resource, &["env"]).await;
        assert_eq!(
            untagged.status, case.status,
            "case {:?} ({}): UntagResource expected HTTP {}, got {} {}",
            case.name,
            case.resource,
            case.status,
            untagged.status,
            untagged.text()
        );
        let code = untagged.error_code();
        assert_eq!(
            code, case.code,
            "case {:?} ({}): UntagResource expected error code {:?}, got {code:?} in {}",
            case.name,
            case.resource,
            case.code,
            untagged.text()
        );
    }

    // No refusal wrote anything: the function those rows qualify still carries exactly
    // the tag set it was created with.
    let expected = tag_set(&[("tier", "gold")]);
    let reported = tags_of(
        "after the refusals",
        &list_tags(&router, &function_arn("published-tags")).await,
    );
    assert_eq!(
        reported, expected,
        "after the refusals: expected tags {expected:?}, got {reported:?}"
    );
}
