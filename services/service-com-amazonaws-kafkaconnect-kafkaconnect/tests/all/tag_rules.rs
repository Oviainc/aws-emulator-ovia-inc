// SPDX-License-Identifier: CC0-1.0

//! The rules a caller-supplied tag has to satisfy before this service stores
//! it, end to end over HTTP.
//!
//! One rule, applied on every path that writes tags: a tag key or value a
//! caller supplies is accepted only when the key stays out of the namespace AWS
//! reserves for the tags it sets itself (a key beginning `aws:`, in any casing)
//! and both the key and the value carry only the published tag character set
//! (letters, numbers, spaces and `+ - = . _ : / @`); anything else is refused
//! with the modelled `BadRequestException` (HTTP 400) and nothing is written.
//!
//! The paths the rule covers are the ones a caller can write tags through:
//! `TagResource`, the `tags` member of `CreateConnector`,
//! `CreateCustomPlugin` and `CreateWorkerConfiguration`, and the `tagKeys` of
//! `UntagResource` — a key AWS reserves is not a caller's to strip either.
//!
//! The tables below span the class the rule generalises over rather than the
//! single recorded case: every character the set admits (each on its own row),
//! letters and numbers outside ASCII, the boundary keys that merely START with
//! `aws` or carry `aws:` somewhere other than the front, each casing of the
//! reserved prefix, and a key and a value carrying each of several characters
//! the set excludes.
//!
//! Every case drives the emulator the way a client does: a buffered `restJson1`
//! request through the same router the production binary assembles. Nothing
//! reaches into the state struct, so a handler that answered from the request
//! rather than from state would fail here.
//!
//! The sibling suites' request helpers are private to their own modules, so this
//! one carries its own copies of the few it needs.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_kafkaconnect_kafkaconnect::EmulatedService;

/// A SigV4 credential scope naming this service and the region every minted ARN
/// must carry. No authenticator is registered, so the signature is never
/// checked; the scope is what routes the request and names the region.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/kafkaconnect/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// The region in [`AUTHORIZATION`].
const REGION: &str = "eu-west-1";

/// The canonical emulator account id every ARN carries.
const ACCOUNT: &str = "000000000000";

/// Base64 of `key.converter=org.apache.kafka.connect.storage.StringConverter`:
/// a legal `propertiesFileContent` for every row, since no row is about the
/// properties file.
const PROPERTIES: &str =
    "a2V5LmNvbnZlcnRlcj1vcmcuYXBhY2hlLmthZmthLmNvbm5lY3Quc3RvcmFnZS5TdHJpbmdDb252ZXJ0ZXI=";

/// The status a successful `TagResource`/`UntagResource` answers (AWS answers
/// these void tag writes 204 No Content).
const TAG_WRITE_STATUS: u16 = 204;

/// The member a refusal of a request's tags blames, and the lead-in of the
/// message it carries: this service answers a parameter rejection
/// `Invalid parameter <member>: <reason>`.
const TAGS_REFUSAL_PREFIX: &str = "Invalid parameter tags: ";

/// The same lead-in for a refusal of a removal's keys.
const TAG_KEYS_REFUSAL_PREFIX: &str = "Invalid parameter tagKeys: ";

/// One emulator: a single shared engine behind the production router.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(engine));
    router
}

/// Send one request and read back `(status, body)`.
async fn call(emulator: &MultiServiceRouter, method: &str, path: &str, body: Value) -> (u16, Value) {
    let bytes = if body.is_null() {
        Vec::new()
    } else {
        serde_json::to_vec(&body).expect("the request body serializes")
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://kafkaconnect.{REGION}.amazonaws.com{path}"))
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(bytes))
        .expect("the request builds");
    let response = emulator.dispatch(request).await;
    let status = response.status().as_u16();
    let body = response.into_body();
    let value = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body).unwrap_or_else(|e| panic!("a JSON response body: {e}"))
    };
    (status, value)
}

/// The single path segment an `@httpLabel` ARN occupies, percent-encoded the way
/// an AWS SDK encodes a label: every character a path segment reserves
/// (including the ARN's own `:` and `/`) is escaped.
fn label(value: &str) -> String {
    percent_encoding::utf8_percent_encode(value, percent_encoding::NON_ALPHANUMERIC).to_string()
}

/// The error code a failure response carries (`__type`), or `""`.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A member of a JSON object as a string, for an assertion message.
fn text(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A tags map for a request body.
fn tags_object(tags: &[(&str, &str)]) -> Value {
    Value::Object(
        tags.iter()
            .map(|(key, value)| ((*key).to_string(), json!(value)))
            .collect(),
    )
}

/// `POST /v1/tags/{arn}` — `TagResource`.
async fn tag(emulator: &MultiServiceRouter, arn: &str, tags: &[(&str, &str)]) -> (u16, Value) {
    let body = json!({"tags": tags_object(tags)});
    call(emulator, "POST", &format!("/v1/tags/{}", label(arn)), body).await
}

/// `DELETE /v1/tags/{arn}?tagKeys=…` — `UntagResource`. `tagKeys` is an
/// `@httpQuery` list, so each key travels as its own repeated `tagKeys=`
/// parameter.
async fn untag(emulator: &MultiServiceRouter, arn: &str, keys: &[&str]) -> (u16, Value) {
    let query: Vec<String> = keys
        .iter()
        .map(|key| format!("tagKeys={}", label(key)))
        .collect();
    call(
        emulator,
        "DELETE",
        &format!("/v1/tags/{}?{}", label(arn), query.join("&")),
        Value::Null,
    )
    .await
}

/// `GET /v1/tags/{arn}` — `ListTagsForResource`, with the `tags` map read back
/// as a sorted map so two tag sets compare irrespective of member order.
async fn list_tags(emulator: &MultiServiceRouter, arn: &str) -> BTreeMap<String, String> {
    let (status, body) = call(
        emulator,
        "GET",
        &format!("/v1/tags/{}", label(arn)),
        Value::Null,
    )
    .await;
    assert_eq!(
        status, 200,
        "ListTagsForResource must succeed, got {status} {body}"
    );
    body.get("tags")
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .map(|(key, value)| (key.clone(), value.as_str().unwrap_or_default().to_string()))
                .collect()
        })
        .unwrap_or_default()
}

/// `POST /v1/worker-configurations` — `CreateWorkerConfiguration`, with `tags`
/// only when some are given (the member is optional).
async fn create_worker_configuration(
    emulator: &MultiServiceRouter,
    name: &str,
    tags: &[(&str, &str)],
) -> (u16, Value) {
    let mut body = json!({"name": name, "propertiesFileContent": PROPERTIES});
    if !tags.is_empty() {
        body["tags"] = tags_object(tags);
    }
    call(emulator, "POST", "/v1/worker-configurations", body).await
}

/// `POST /v1/custom-plugins` — `CreateCustomPlugin`.
async fn create_custom_plugin(
    emulator: &MultiServiceRouter,
    name: &str,
    tags: &[(&str, &str)],
) -> (u16, Value) {
    let mut body = json!({
        "name": name,
        "contentType": "ZIP",
        "location": {"s3Location": {
            "bucketArn": "arn:aws:s3:::plugin-bucket",
            "fileKey": "plugins/p.zip",
        }},
    });
    if !tags.is_empty() {
        body["tags"] = tags_object(tags);
    }
    call(emulator, "POST", "/v1/custom-plugins", body).await
}

/// `POST /v1/connectors` — `CreateConnector`, on a plugin ARN this store holds.
async fn create_connector(
    emulator: &MultiServiceRouter,
    name: &str,
    plugin_arn: &str,
    tags: &[(&str, &str)],
) -> (u16, Value) {
    let mut body = json!({
        "connectorName": name,
        "capacity": {"provisionedCapacity": {"mcuCount": 1, "workerCount": 2}},
        "connectorConfiguration": {"tasks.max": "1"},
        "kafkaCluster": {"apacheKafkaCluster": {
            "bootstrapServers": "b-1.example:9098",
            "vpc": {"subnets": ["subnet-1", "subnet-2"]},
        }},
        "kafkaClusterClientAuthentication": {"authenticationType": "IAM"},
        "kafkaClusterEncryptionInTransit": {"encryptionType": "TLS"},
        "kafkaConnectVersion": "2.7.1",
        "plugins": [{"customPlugin": {"customPluginArn": plugin_arn, "revision": 1}}],
        "serviceExecutionRoleArn": format!("arn:aws:iam::{ACCOUNT}:role/connector-role"),
    });
    if !tags.is_empty() {
        body["tags"] = tags_object(tags);
    }
    call(emulator, "POST", "/v1/connectors", body).await
}

/// Create one worker configuration that must succeed, and return its ARN.
async fn a_worker_configuration(emulator: &MultiServiceRouter, name: &str) -> String {
    let (status, created) = create_worker_configuration(emulator, name, &[]).await;
    assert_eq!(
        status, 200,
        "case {name:?}: CreateWorkerConfiguration must succeed, got {status} {created}"
    );
    text(&created, "workerConfigurationArn")
}

/// Whether the rule admits a row's key and value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Verdict {
    /// The tag is one a caller may write: it is stored and reads back.
    Stored,
    /// The tag breaks the rule: HTTP 400 and nothing written.
    Refused,
}

/// One row of the tag table: a key, a value, and what the rule says about them.
#[derive(Clone, Copy, Debug)]
struct TagCase {
    case: &'static str,
    key: &'static str,
    value: &'static str,
    verdict: Verdict,
}

/// The tag table.
///
/// The accepted rows walk the published tag character set one character at a
/// time — letters, numbers, spaces and `+ - = . _ : / @`, with letters and
/// numbers taken as the Unicode categories, since the set is the characters
/// "representable in UTF-8" rather than the ASCII subset — plus the boundary
/// keys around the reserved prefix. The refused rows carry the reserved prefix
/// in each casing, or a character the set excludes, in the key or in the value.
fn tag_cases() -> Vec<TagCase> {
    vec![
        TagCase {
            case: "a plain key and value",
            key: "env",
            value: "probe",
            verdict: Verdict::Stored,
        },
        TagCase {
            case: "a plus in the key and the value",
            key: "plus+key",
            value: "a+b",
            verdict: Verdict::Stored,
        },
        TagCase {
            case: "a hyphen in the key and the value",
            key: "minus-key",
            value: "a-b",
            verdict: Verdict::Stored,
        },
        TagCase {
            case: "an equals sign in the key and the value",
            key: "equals=key",
            value: "a=b",
            verdict: Verdict::Stored,
        },
        TagCase {
            case: "a dot in the key and the value",
            key: "dot.key",
            value: "a.b",
            verdict: Verdict::Stored,
        },
        TagCase {
            case: "an underscore in the key and the value",
            key: "under_key",
            value: "a_b",
            verdict: Verdict::Stored,
        },
        TagCase {
            case: "a colon that is not the reserved prefix",
            key: "team:owner",
            value: "a:b",
            verdict: Verdict::Stored,
        },
        TagCase {
            case: "a slash in the key and the value",
            key: "slash/key",
            value: "a/b",
            verdict: Verdict::Stored,
        },
        TagCase {
            case: "an at sign in the key and the value",
            key: "at@key",
            value: "a@b",
            verdict: Verdict::Stored,
        },
        TagCase {
            case: "spaces in the key and the value",
            key: "two words",
            value: "with a space",
            verdict: Verdict::Stored,
        },
        TagCase {
            case: "numbers in the key and the value",
            key: "digits123",
            value: "456",
            verdict: Verdict::Stored,
        },
        TagCase {
            case: "letters outside ASCII",
            key: "clé-日本語",
            value: "значение",
            verdict: Verdict::Stored,
        },
        TagCase {
            case: "an empty value",
            key: "empty-value",
            value: "",
            verdict: Verdict::Stored,
        },
        TagCase {
            // The prefix is `aws:`: a key that merely begins with those three
            // letters claims nothing.
            case: "a key beginning with the letters of the prefix but no colon",
            key: "awsome",
            value: "not reserved",
            verdict: Verdict::Stored,
        },
        TagCase {
            case: "a key that is exactly the three letters",
            key: "aws",
            value: "not reserved",
            verdict: Verdict::Stored,
        },
        TagCase {
            case: "the prefix somewhere other than the front of the key",
            key: "team:aws:owner",
            value: "not reserved",
            verdict: Verdict::Stored,
        },
        TagCase {
            case: "the reserved prefix, lowercase",
            key: "aws:cloudformation:stack-name",
            value: "mine",
            verdict: Verdict::Refused,
        },
        TagCase {
            case: "the reserved prefix, uppercase",
            key: "AWS:Region",
            value: "eu-west-1",
            verdict: Verdict::Refused,
        },
        TagCase {
            case: "the reserved prefix, mixed case",
            key: "Aws:createdBy",
            value: "someone",
            verdict: Verdict::Refused,
        },
        TagCase {
            case: "the reserved prefix and nothing else",
            key: "aws:",
            value: "",
            verdict: Verdict::Refused,
        },
        TagCase {
            case: "an exclamation mark in the key",
            key: "bad key!",
            value: "v",
            verdict: Verdict::Refused,
        },
        TagCase {
            case: "a hash in the key",
            key: "key#1",
            value: "v",
            verdict: Verdict::Refused,
        },
        TagCase {
            case: "a comma in the key",
            key: "comma,key",
            value: "v",
            verdict: Verdict::Refused,
        },
        TagCase {
            case: "a brace in the key",
            key: "brace{key}",
            value: "v",
            verdict: Verdict::Refused,
        },
        TagCase {
            case: "a percent sign in the key",
            key: "percent%key",
            value: "v",
            verdict: Verdict::Refused,
        },
        TagCase {
            case: "a tab in the key",
            key: "tab\tkey",
            value: "v",
            verdict: Verdict::Refused,
        },
        TagCase {
            case: "an exclamation mark in the value",
            key: "legal-key-1",
            value: "no!",
            verdict: Verdict::Refused,
        },
        TagCase {
            case: "a semicolon in the value",
            key: "legal-key-2",
            value: "semi;colon",
            verdict: Verdict::Refused,
        },
        TagCase {
            case: "a newline in the value",
            key: "legal-key-3",
            value: "two\nlines",
            verdict: Verdict::Refused,
        },
    ]
}

/// Assert a refusal is the modelled 400 blaming `prefix`'s member, and that it
/// goes on to give a reason. The reason sentence is this emulator's own wording,
/// so the assertion fixes the part the rule fixes — the status, the error code
/// and the member — rather than re-stating a literal the handler carries.
fn assert_refused(case: &str, prefix: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} {body}"
    );
    assert!(
        error_code(body).contains("BadRequestException"),
        "case {case:?}: expected BadRequestException, got {body}"
    );
    let message = text(body, "message");
    assert!(
        message.starts_with(prefix) && message.len() > prefix.len(),
        "case {case:?}: expected a message starting {prefix:?} and carrying a reason, got \
         {message:?}"
    );
}

#[tokio::test]
async fn tag_resource_stores_only_the_tags_aws_accepts() {
    let emulator = emulator();
    let arn = a_worker_configuration(&emulator, "wc-tag-rules").await;

    let mut stored: BTreeMap<String, String> = BTreeMap::new();
    for case in tag_cases() {
        let (status, answered) = tag(&emulator, &arn, &[(case.key, case.value)]).await;
        match case.verdict {
            Verdict::Stored => {
                assert_eq!(
                    status, TAG_WRITE_STATUS,
                    "case {:?}: expected HTTP {TAG_WRITE_STATUS}, got {status} {answered}",
                    case.case
                );
                stored.insert(case.key.to_string(), case.value.to_string());
            }
            Verdict::Refused => {
                assert_refused(case.case, TAGS_REFUSAL_PREFIX, status, &answered);
            }
        }

        // Whatever the verdict, the stored set is exactly the accepted rows so
        // far: an accepted tag reads back unchanged, and a refused one leaves
        // the resource's tag map as it was — nothing written.
        let actual = list_tags(&emulator, &arn).await;
        assert_eq!(
            actual, stored,
            "case {:?}: expected the tag set {stored:?}, got {actual:?}",
            case.case
        );
    }
}

#[tokio::test]
async fn a_tag_the_rule_refuses_is_refused_whichever_member_of_a_request_carries_it() {
    // The same rule on the `tags` of each create: a refused create mints
    // nothing, which the name being free afterwards proves.
    let emulator = emulator();

    for case in tag_cases().into_iter().filter(|case| case.verdict == Verdict::Refused) {
        let tags = [(case.key, case.value)];

        let (status, answered) =
            create_worker_configuration(&emulator, "wc-create-tag-rules", &tags).await;
        assert_refused(case.case, TAGS_REFUSAL_PREFIX, status, &answered);

        let (status, answered) =
            create_custom_plugin(&emulator, "cp-create-tag-rules", &tags).await;
        assert_refused(case.case, TAGS_REFUSAL_PREFIX, status, &answered);
    }

    // Nothing above was created, so every one of those names is still free and
    // a create carrying only legal tags succeeds and reads back.
    let (status, created) = create_worker_configuration(
        &emulator,
        "wc-create-tag-rules",
        &[("env", "probe"), ("awsome", "not reserved")],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"a create with legal tags\": expected HTTP 200, got {status} {created}"
    );
    let worker_arn = text(&created, "workerConfigurationArn");
    let (status, described) = call(
        &emulator,
        "GET",
        &format!("/v1/worker-configurations/{}", label(&worker_arn)),
        Value::Null,
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"a create with legal tags\": the configuration must read back, got {status} \
         {described}"
    );
    assert_eq!(
        text(&described, "name"),
        "wc-create-tag-rules",
        "case \"a create with legal tags\": the name must be the one created, got {described}"
    );
    let worker_tags = list_tags(&emulator, &worker_arn).await;
    assert_eq!(
        worker_tags,
        BTreeMap::from([
            ("env".to_string(), "probe".to_string()),
            ("awsome".to_string(), "not reserved".to_string()),
        ]),
        "case \"a create with legal tags\": expected the created tags, got {worker_tags:?}"
    );

    let (status, plugin) = create_custom_plugin(
        &emulator,
        "cp-create-tag-rules",
        &[("team:owner", "data eng")],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"a plugin with legal tags\": expected HTTP 200, got {status} {plugin}"
    );
    let plugin_arn = text(&plugin, "customPluginArn");
    let plugin_tags = list_tags(&emulator, &plugin_arn).await;
    assert_eq!(
        plugin_tags,
        BTreeMap::from([("team:owner".to_string(), "data eng".to_string())]),
        "case \"a plugin with legal tags\": expected the created tags, got {plugin_tags:?}"
    );

    // And the same on a connector's `tags`, the third create that carries them.
    let (status, answered) = create_connector(
        &emulator,
        "connector-tag-rules",
        &plugin_arn,
        &[("aws:cloudformation:stack-name", "mine")],
    )
    .await;
    assert_refused(
        "a connector created with the reserved prefix",
        TAGS_REFUSAL_PREFIX,
        status,
        &answered,
    );
    let (status, listed) = call(&emulator, "GET", "/v1/connectors", Value::Null).await;
    assert_eq!(
        status, 200,
        "ListConnectors must succeed, got {status} {listed}"
    );
    let connectors = listed
        .get("connectors")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or_default();
    assert_eq!(
        connectors, 0,
        "case \"a connector created with the reserved prefix\": a refused create mints nothing, \
         got {listed}"
    );
}

#[tokio::test]
async fn untag_resource_refuses_a_key_a_caller_could_not_have_written() {
    let emulator = emulator();
    let arn = a_worker_configuration(&emulator, "wc-untag-rules").await;

    let legal = [("env", "probe"), ("team:owner", "data eng")];
    let (status, answered) = tag(&emulator, &arn, &legal).await;
    assert_eq!(
        status, TAG_WRITE_STATUS,
        "the legal tags must be stored, got {status} {answered}"
    );
    let before = list_tags(&emulator, &arn).await;

    // A removal names caller-supplied keys, so the same rule holds: a key in
    // the reserved namespace or outside the character set is refused, and
    // nothing is removed — including the legal keys a mixed request names.
    let refused: Vec<(&str, Vec<&str>)> = vec![
        ("a reserved key alone", vec!["aws:cloudformation:stack-name"]),
        ("a reserved key in another casing", vec!["AWS:Region"]),
        ("the reserved prefix and nothing else", vec!["aws:"]),
        (
            "a reserved key beside a key the resource carries",
            vec!["env", "aws:cloudformation:stack-name"],
        ),
        ("a key outside the character set", vec!["bad key!"]),
        (
            "a key outside the character set beside a legal one",
            vec!["team:owner", "key#1"],
        ),
    ];

    for (case, keys) in &refused {
        let (status, answered) = untag(&emulator, &arn, keys).await;
        assert_refused(case, TAG_KEYS_REFUSAL_PREFIX, status, &answered);
        let actual = list_tags(&emulator, &arn).await;
        assert_eq!(
            actual, before,
            "case {case:?}: a refused removal must remove nothing, expected {before:?}, got \
             {actual:?}"
        );
    }

    // A removal naming only keys a caller may hold still works: the rule
    // refuses illegal keys, not every removal.
    let (status, answered) = untag(&emulator, &arn, &["env", "awsome"]).await;
    assert_eq!(
        status, TAG_WRITE_STATUS,
        "case \"a legal removal\": expected HTTP {TAG_WRITE_STATUS}, got {status} {answered}"
    );
    let actual = list_tags(&emulator, &arn).await;
    assert_eq!(
        actual,
        BTreeMap::from([("team:owner".to_string(), "data eng".to_string())]),
        "case \"a legal removal\": expected only the unnamed key to survive, got {actual:?}"
    );
}
