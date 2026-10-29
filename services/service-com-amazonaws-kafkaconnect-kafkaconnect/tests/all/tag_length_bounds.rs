// SPDX-License-Identifier: CC0-1.0

//! How long a caller-supplied tag key or value may be, end to end over HTTP.
//!
//! One rule, applied wherever a request names a tag: a tag key or value is
//! accepted only when the number of CHARACTERS it holds lies inside the
//! `@length` the model declares for its shape, and a request naming one outside
//! it is refused as a modelled-constraint violation — the modelled
//! `BadRequestException` (HTTP 400), whose message states the clause the
//! framework's own validating walk renders — with nothing written, read back or
//! removed.
//!
//! The bounds are the model's, not this suite's:
//! `smithy_query(query="shape", shape="Tags,TagKey,TagValue")` reports `TagKey`
//! `length 1..128` and `TagValue` `length 0..256`, which the `Tags` map repeats
//! on its key and value, and `UntagResource$tagKeys` is a list of that same
//! `TagKey`. [`the_tag_key_bounds_this_suite_asserts_are_the_ones_the_service_holds`]
//! ties the service's own constants to those numbers, so neither side can drift
//! from the model on its own.
//!
//! The paths the rule covers are the ones a caller can name a tag through:
//! `TagResource`, the `tags` member of `CreateWorkerConfiguration`,
//! `CreateCustomPlugin` and `CreateConnector`, and the `tagKeys` of
//! `UntagResource`. The three positions a tag's text can occupy are reported at
//! the three positions the walk spells: a map's KEY at the member that carried
//! the map, a map's VALUE at that member plus the entry's key, and a list
//! element at its 1-based position.
//!
//! The table spans the class the rule generalises over rather than one cited
//! case: each published bound, one character short of a minimum and one past a
//! maximum, far past a maximum, both halves of an entry at once, and keys and
//! values built from multi-byte characters — a length is a count of characters,
//! so a key of 128 three-byte characters is inside the bound and one of 129 is
//! not.
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

use service_com_amazonaws_kafkaconnect_kafkaconnect::state::{
    MAX_TAG_KEY_LENGTH, MIN_TAG_KEY_LENGTH,
};
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

/// The lead-in of the message this service refuses a request that broke a
/// MODELLED constraint with, whichever member broke it.
const CONSTRAINT_REFUSAL_PREFIX: &str = "Bad request due to incorrect input: ";

/// `TagKey`'s modelled minimum length. Source: `smithy_query(query="shape",
/// shape="Tags,TagKey,TagValue")` — `TagKey` is `length 1..128`.
const MODELLED_KEY_MIN: usize = 1;

/// `TagKey`'s modelled maximum length, from that same shape.
const MODELLED_KEY_MAX: usize = 128;

/// `TagValue`'s modelled maximum length (same query) — `TagValue` is
/// `length 0..256`, whose minimum of 0 makes an empty value a tag.
const MODELLED_VALUE_MAX: usize = 256;

/// A character that takes three bytes in UTF-8, for the rows that separate a
/// count of CHARACTERS from a count of bytes.
const WIDE: char = '語';

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
/// an AWS SDK encodes a label.
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

/// A member of a JSON object as a string.
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

/// `GET /v1/tags/{arn}` — `ListTagsForResource`, read back as a sorted map so
/// two tag sets compare irrespective of member order.
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

/// Create one custom plugin that must succeed, and return its ARN.
async fn a_custom_plugin(emulator: &MultiServiceRouter, name: &str) -> String {
    let (status, created) = create_custom_plugin(emulator, name, &[]).await;
    assert_eq!(
        status, 200,
        "case {name:?}: CreateCustomPlugin must succeed, got {status} {created}"
    );
    text(&created, "customPluginArn")
}

/// A string of `count` repetitions of `filler`.
fn run_of(filler: char, count: usize) -> String {
    filler.to_string().repeat(count)
}

/// The requirement clause a value shorter than a modelled minimum fails.
fn at_least(bound: usize) -> String {
    format!("Member must have length greater than or equal to {bound}")
}

/// The requirement clause a value longer than a modelled maximum fails.
fn at_most(bound: usize) -> String {
    format!("Member must have length less than or equal to {bound}")
}

/// Whether the rule admits a row's key and value, and — when it does not — the
/// clause the refusal has to state.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Verdict {
    /// Both halves are inside their bound: the tag is stored and reads back.
    Stored,
    /// A half is outside its bound: HTTP 400 and nothing written, with a clause
    /// naming the position the walk reports that half at and the requirement it
    /// failed.
    Refused { at: String, requirement: String },
}

/// One row of the length table.
#[derive(Clone, Debug)]
struct LengthCase {
    case: String,
    key: String,
    value: String,
    verdict: Verdict,
}

/// A refusal of a tag KEY: a map key is a value the caller chose rather than a
/// member of its own, so the clause names the member that carried the map.
fn key_refusal(requirement: String) -> Verdict {
    Verdict::Refused {
        at: "tags".to_string(),
        requirement,
    }
}

/// A refusal of a tag VALUE, which the walk reports at the member that carried
/// the map plus the entry's own key.
fn value_refusal(key: &str, requirement: String) -> Verdict {
    Verdict::Refused {
        at: format!("tags.{key}"),
        requirement,
    }
}

/// One row of the batch table: the whole request's entries, and the verdict
/// the rule gives that request.
struct BatchCase<'a> {
    case: &'a str,
    entries: Vec<(&'a str, &'a str)>,
    verdict: Verdict,
}

/// The length table: every published bound of both halves, one character
/// outside each of them, far outside a maximum, both halves outside at once,
/// and the same bounds measured in multi-byte characters.
fn length_cases() -> Vec<LengthCase> {
    let key_past_max = run_of('k', MODELLED_KEY_MAX + 1);
    vec![
        LengthCase {
            case: format!("a key at the modelled minimum of {MODELLED_KEY_MIN}"),
            key: run_of('k', MODELLED_KEY_MIN),
            value: "legal-value".to_string(),
            verdict: Verdict::Stored,
        },
        LengthCase {
            case: format!("a key one character short of the minimum of {MODELLED_KEY_MIN}"),
            key: run_of('k', MODELLED_KEY_MIN - 1),
            value: "legal-value".to_string(),
            verdict: key_refusal(at_least(MODELLED_KEY_MIN)),
        },
        LengthCase {
            case: format!("a key at the modelled maximum of {MODELLED_KEY_MAX}"),
            key: run_of('k', MODELLED_KEY_MAX),
            value: "legal-value".to_string(),
            verdict: Verdict::Stored,
        },
        LengthCase {
            case: format!("a key one character past the maximum of {MODELLED_KEY_MAX}"),
            key: key_past_max.clone(),
            value: "legal-value".to_string(),
            verdict: key_refusal(at_most(MODELLED_KEY_MAX)),
        },
        LengthCase {
            case: "a key far past the maximum".to_string(),
            key: run_of('k', MODELLED_KEY_MAX * 8),
            value: "legal-value".to_string(),
            verdict: key_refusal(at_most(MODELLED_KEY_MAX)),
        },
        LengthCase {
            case: "a value at the modelled minimum of 0".to_string(),
            key: "value-at-min".to_string(),
            value: String::new(),
            verdict: Verdict::Stored,
        },
        LengthCase {
            case: format!("a value at the modelled maximum of {MODELLED_VALUE_MAX}"),
            key: "value-at-max".to_string(),
            value: run_of('v', MODELLED_VALUE_MAX),
            verdict: Verdict::Stored,
        },
        LengthCase {
            case: format!("a value one character past the maximum of {MODELLED_VALUE_MAX}"),
            key: "value-past-max".to_string(),
            value: run_of('v', MODELLED_VALUE_MAX + 1),
            verdict: value_refusal("value-past-max", at_most(MODELLED_VALUE_MAX)),
        },
        LengthCase {
            case: "a value far past the maximum".to_string(),
            key: "value-far-past-max".to_string(),
            value: run_of('v', MODELLED_VALUE_MAX * 8),
            verdict: value_refusal("value-far-past-max", at_most(MODELLED_VALUE_MAX)),
        },
        LengthCase {
            case: "both halves past their maxima".to_string(),
            key: key_past_max.clone(),
            value: run_of('v', MODELLED_VALUE_MAX + 1),
            // A modelled constraint is decided at the request boundary, before
            // the handler reads the request at all, so the half the boundary
            // reaches — the entry's value — is the one this clause names.
            verdict: value_refusal(&key_past_max, at_most(MODELLED_VALUE_MAX)),
        },
        LengthCase {
            case: format!("a key of {MODELLED_KEY_MAX} multi-byte characters"),
            key: run_of(WIDE, MODELLED_KEY_MAX),
            value: "legal-value".to_string(),
            verdict: Verdict::Stored,
        },
        LengthCase {
            case: format!("a key of {} multi-byte characters", MODELLED_KEY_MAX + 1),
            key: run_of(WIDE, MODELLED_KEY_MAX + 1),
            value: "legal-value".to_string(),
            verdict: key_refusal(at_most(MODELLED_KEY_MAX)),
        },
        LengthCase {
            case: format!("a value of {MODELLED_VALUE_MAX} multi-byte characters"),
            key: "wide-value-at-max".to_string(),
            value: run_of(WIDE, MODELLED_VALUE_MAX),
            verdict: Verdict::Stored,
        },
        LengthCase {
            case: format!(
                "a value of {} multi-byte characters",
                MODELLED_VALUE_MAX + 1
            ),
            key: "wide-value-past-max".to_string(),
            value: run_of(WIDE, MODELLED_VALUE_MAX + 1),
            verdict: value_refusal("wide-value-past-max", at_most(MODELLED_VALUE_MAX)),
        },
    ]
}

/// Assert a refusal is the modelled 400 whose message states the clause for the
/// position `at` and the requirement `requirement`.
///
/// The clause is the framework's own rendering of a modelled constraint, which
/// is what every other length-bearing member of this service is refused with,
/// so the assertion fixes what the rule fixes: the status, the error code, the
/// position the violation is reported at, and the bound the model states.
fn assert_refused(case: &str, at: &str, requirement: &str, status: u16, body: &Value) {
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
        message.starts_with(CONSTRAINT_REFUSAL_PREFIX),
        "case {case:?}: expected a message starting {CONSTRAINT_REFUSAL_PREFIX:?}, got {message:?}"
    );
    let clause = format!("at '{at}' failed to satisfy constraint: {requirement}");
    assert!(
        message.contains(&clause),
        "case {case:?}: expected the message to state {clause:?}, got {message:?}"
    );
}

/// Assert a row's verdict for one response.
fn assert_verdict(case: &str, verdict: &Verdict, status: u16, body: &Value) {
    match verdict {
        Verdict::Stored => panic!("case {case:?}: a stored row has no refusal to assert"),
        Verdict::Refused { at, requirement } => {
            assert_refused(case, at, requirement, status, body);
        }
    }
}

#[test]
fn the_tag_key_bounds_this_suite_asserts_are_the_ones_the_service_holds() {
    // The numbers the key half of the rule is about come from the model (see the
    // module note); the service states them once, and this is where the two are
    // tied together, so neither can drift from the model on its own. The value
    // half's bound is the model's too, enforced by the shared boundary walk off
    // the generated schema, so there is no second copy of it to tie.
    for (case, modelled, held) in [
        ("TagKey minimum", MODELLED_KEY_MIN, MIN_TAG_KEY_LENGTH),
        ("TagKey maximum", MODELLED_KEY_MAX, MAX_TAG_KEY_LENGTH),
    ] {
        assert_eq!(
            held, modelled,
            "case {case:?}: expected the modelled bound {modelled}, got {held}"
        );
    }
}

#[tokio::test]
async fn tag_resource_stores_only_a_key_and_value_inside_their_modelled_length() {
    let emulator = emulator();
    let arn = a_worker_configuration(&emulator, "wc-tag-lengths").await;

    let mut stored: BTreeMap<String, String> = BTreeMap::new();
    for case in length_cases() {
        let (status, answered) = tag(&emulator, &arn, &[(&case.key, &case.value)]).await;
        match &case.verdict {
            Verdict::Stored => {
                assert_eq!(
                    status, TAG_WRITE_STATUS,
                    "case {:?}: expected HTTP {TAG_WRITE_STATUS}, got {status} {answered}",
                    case.case
                );
                stored.insert(case.key.clone(), case.value.clone());
            }
            verdict => assert_verdict(&case.case, verdict, status, &answered),
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
async fn one_entry_outside_its_bound_refuses_the_whole_request() {
    // A tag write is all or nothing: a request carrying several entries of which
    // ONE is outside its bound writes none of them, and the entries the resource
    // already holds keep the values they had.
    let emulator = emulator();
    let arn = a_worker_configuration(&emulator, "wc-tag-lengths-batch").await;

    let (status, answered) = tag(&emulator, &arn, &[("env", "probe"), ("team", "data")]).await;
    assert_eq!(
        status, TAG_WRITE_STATUS,
        "case \"a legal batch\": expected HTTP {TAG_WRITE_STATUS}, got {status} {answered}"
    );
    let before = list_tags(&emulator, &arn).await;

    let key_past_max = run_of('k', MODELLED_KEY_MAX + 1);
    let value_past_max = run_of('v', MODELLED_VALUE_MAX + 1);
    let rows = vec![
        BatchCase {
            case: "a legal entry beside a key past its bound",
            entries: vec![("env", "changed"), (key_past_max.as_str(), "v")],
            verdict: key_refusal(at_most(MODELLED_KEY_MAX)),
        },
        BatchCase {
            case: "a legal entry beside a value past its bound",
            entries: vec![("env", "changed"), ("other", value_past_max.as_str())],
            verdict: value_refusal("other", at_most(MODELLED_VALUE_MAX)),
        },
        BatchCase {
            case: "a legal entry beside an empty key",
            entries: vec![("env", "changed"), ("", "v")],
            verdict: key_refusal(at_least(MODELLED_KEY_MIN)),
        },
    ];

    for row in rows {
        let (status, answered) = tag(&emulator, &arn, &row.entries).await;
        assert_verdict(row.case, &row.verdict, status, &answered);
        let actual = list_tags(&emulator, &arn).await;
        assert_eq!(
            actual, before,
            "case {:?}: a refused write changes nothing, expected {before:?}, got {actual:?}",
            row.case
        );
    }
}

#[tokio::test]
async fn a_tag_outside_its_bound_is_refused_whichever_member_of_a_request_carries_it() {
    // The same rule on the `tags` of each create: a refused create mints
    // nothing, which the name being free afterwards proves.
    let emulator = emulator();
    let plugin = a_custom_plugin(&emulator, "cp-tag-length-host").await;

    for case in length_cases()
        .into_iter()
        .filter(|case| case.verdict != Verdict::Stored)
    {
        let tags = [(case.key.as_str(), case.value.as_str())];

        let (status, answered) =
            create_worker_configuration(&emulator, "wc-create-tag-lengths", &tags).await;
        assert_verdict(&case.case, &case.verdict, status, &answered);

        let (status, answered) =
            create_custom_plugin(&emulator, "cp-create-tag-lengths", &tags).await;
        assert_verdict(&case.case, &case.verdict, status, &answered);

        let (status, answered) =
            create_connector(&emulator, "co-create-tag-lengths", &plugin, &tags).await;
        assert_verdict(&case.case, &case.verdict, status, &answered);
    }

    // Nothing above was created, so each of those names is still free, and a
    // create carrying tags at the bounds succeeds and reads them back.
    let key_at_max = run_of('k', MODELLED_KEY_MAX);
    let value_at_max = run_of('v', MODELLED_VALUE_MAX);
    let (status, created) = create_worker_configuration(
        &emulator,
        "wc-create-tag-lengths",
        &[
            (key_at_max.as_str(), "v"),
            ("value-at-max", value_at_max.as_str()),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"a create with tags at the bounds\": expected HTTP 200, got {status} {created}"
    );
    let arn = text(&created, "workerConfigurationArn");
    let actual = list_tags(&emulator, &arn).await;
    assert_eq!(
        actual,
        BTreeMap::from([
            (key_at_max.clone(), "v".to_string()),
            ("value-at-max".to_string(), value_at_max.clone()),
        ]),
        "case \"a create with tags at the bounds\": expected both tags to read back, got \
         {actual:?}"
    );
}

#[tokio::test]
async fn untag_resource_refuses_a_key_outside_the_modelled_key_length() {
    // A removal names keys only, so only the key half of the rule applies to
    // it — reported at the key's own position in the list it travelled in — and
    // a request naming one key outside the bound removes nothing at all.
    let emulator = emulator();
    let arn = a_worker_configuration(&emulator, "wc-untag-lengths").await;

    let key_at_min = run_of('k', MODELLED_KEY_MIN);
    let key_at_max = run_of('k', MODELLED_KEY_MAX);
    let (status, answered) = tag(
        &emulator,
        &arn,
        &[(key_at_min.as_str(), "a"), (key_at_max.as_str(), "b")],
    )
    .await;
    assert_eq!(
        status, TAG_WRITE_STATUS,
        "case \"keys at both bounds\": expected HTTP {TAG_WRITE_STATUS}, got {status} {answered}"
    );
    let before = list_tags(&emulator, &arn).await;

    let key_past_max = run_of('k', MODELLED_KEY_MAX + 1);
    let wide_past_max = run_of(WIDE, MODELLED_KEY_MAX + 1);
    let rows: Vec<(&str, Vec<&str>, String, String)> = vec![
        (
            "a key one character past the maximum",
            vec![key_past_max.as_str()],
            "tagKeys.1.member".to_string(),
            at_most(MODELLED_KEY_MAX),
        ),
        (
            "a key of too many multi-byte characters",
            vec![wide_past_max.as_str()],
            "tagKeys.1.member".to_string(),
            at_most(MODELLED_KEY_MAX),
        ),
        (
            "an empty key",
            vec![""],
            "tagKeys.1.member".to_string(),
            at_least(MODELLED_KEY_MIN),
        ),
        (
            "a stored key named beside one past the maximum",
            vec![key_at_min.as_str(), key_past_max.as_str()],
            // The second element of the list, named by its own 1-based
            // position, so a caller naming several keys learns which one.
            "tagKeys.2.member".to_string(),
            at_most(MODELLED_KEY_MAX),
        ),
    ];

    for (case, keys, at, requirement) in rows {
        let (status, answered) = untag(&emulator, &arn, &keys).await;
        assert_refused(case, &at, &requirement, status, &answered);
        let actual = list_tags(&emulator, &arn).await;
        assert_eq!(
            actual, before,
            "case {case:?}: a refused removal removes nothing, expected {before:?}, got {actual:?}"
        );
    }

    // A key INSIDE the bound still goes, whatever its length: the rule refuses
    // what the model excludes, not what is merely long.
    let (status, answered) = untag(&emulator, &arn, &[key_at_max.as_str()]).await;
    assert_eq!(
        status, TAG_WRITE_STATUS,
        "case \"a key at the maximum\": expected HTTP {TAG_WRITE_STATUS}, got {status} {answered}"
    );
    let actual = list_tags(&emulator, &arn).await;
    assert_eq!(
        actual,
        BTreeMap::from([(key_at_min.clone(), "a".to_string())]),
        "case \"a key at the maximum\": expected only the named key gone, got {actual:?}"
    );
}

#[tokio::test]
async fn a_refusal_names_every_key_outside_the_bound_in_one_stable_message() {
    // The entries of a `tags` map reach a handler unordered, so a request
    // breaking the bound in more than one place is answered by ONE message that
    // names each of them in key order — the one order the entries themselves
    // fix — rather than by whichever entry the map happened to iterate first.
    let emulator = emulator();
    let arn = a_worker_configuration(&emulator, "wc-tag-length-order").await;

    let first = format!("aaa{}", run_of('k', MODELLED_KEY_MAX));
    let second = format!("bbb{}", run_of('k', MODELLED_KEY_MAX));
    let mut answers: Vec<String> = Vec::new();
    for attempt in 1..=16 {
        let (status, answered) = tag(
            &emulator,
            &arn,
            &[
                (second.as_str(), "v"),
                (first.as_str(), "v"),
                ("env", "probe"),
            ],
        )
        .await;
        let case = format!("two keys past the maximum, attempt {attempt}");
        assert_refused(
            &case,
            "tags",
            &at_most(MODELLED_KEY_MAX),
            status,
            &answered,
        );
        let message = text(&answered, "message");
        let clauses = message.matches("failed to satisfy constraint").count();
        assert_eq!(
            clauses, 2,
            "case {case:?}: expected one clause per key outside the bound, got {message:?}"
        );
        let (left, right) = (
            message.find(&first[..8]),
            message.find(&second[..8]),
        );
        assert!(
            left.is_some() && right.is_some() && left < right,
            "case {case:?}: expected both keys named in key order, got {message:?}"
        );
        answers.push(message);
    }
    assert!(
        answers.windows(2).all(|pair| pair[0] == pair[1]),
        "expected one refusal text for one request, got {answers:?}"
    );

    let actual = list_tags(&emulator, &arn).await;
    assert!(
        actual.is_empty(),
        "no refused write may store anything, got {actual:?}"
    );
}
