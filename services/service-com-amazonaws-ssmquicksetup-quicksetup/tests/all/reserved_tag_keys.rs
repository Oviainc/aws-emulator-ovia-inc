// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the reserved tag-key prefix.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization, the modelled-constraint walk and the handlers — so what is
//! asserted is what a signed AWS SDK request observes.
//!
//! The rule under test: a tag key under the Amazon Web Services-reserved prefix is not
//! the caller's to assign, edit or delete, so EVERY member that takes tag keys from the
//! caller — `CreateConfigurationManager$Tags`, `TagResource$Tags` and
//! `UntagResource$TagKeys` — refuses a request naming one with a 400
//! `ValidationException` and leaves the stored tags exactly as they were. The prefix is
//! reserved however its letters are cased, while a key that merely RESEMBLES it is an
//! ordinary key the service stores, reports and removes like any other.
//!
//! The tables span what the rule generalises over: the casing of the prefix, what
//! follows it (nothing, a plain tail, a punctuated tail, a multi-byte tail), where the
//! prefix sits in the key, how many keys one request names, and which operation carries
//! them. Both readers a stored tag becomes visible through — `GetConfigurationManager`
//! and `ListTagsForResource` — are checked, since a refused write must leave neither
//! reporting a reserved key.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Map, Value};
use service_com_amazonaws_ssmquicksetup_quicksetup::state::{
    QUICK_SETUP_TYPE_CATALOG, RESERVED_TAG_PREFIX,
};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below signs for.
const REGION: &str = "us-west-2";

/// A well-formed `ManagerArn` (the modelled pattern ends in a uuid) that no create ever
/// minted, for the case that pits request validation against resource resolution.
const UNKNOWN_MANAGER_ARN: &str =
    "arn:aws:ssm-quicksetup:us-west-2:000000000000:configuration-manager/\
     00000000-0000-4000-8000-000000000000";

/// One emulator with an empty store, mounted exactly as the router mounts it.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_ssmquicksetup_quicksetup::EmulatedService::new(engine),
    );
    router
}

/// A request signed for `ssm-quicksetup` in [`REGION`], the signal the router routes on
/// and the handlers read the region from.
fn request(method: &str, uri: &str, body: Option<&Value>) -> http::Request<bytes::Bytes> {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("ssm-quicksetup.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDQUICKSETUP/20260101/{REGION}/ssm-quicksetup/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        );
    let payload = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            bytes::Bytes::from(serde_json::to_vec(value).expect("serialize the request body"))
        }
        None => bytes::Bytes::new(),
    };
    builder.body(payload).expect("a well-formed test request")
}

/// Status and parsed body of one call. An empty body reads as `Value::Null`.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let response = router.dispatch(request(method, uri, body)).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let raw = response.into_body();
    let mut parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    // restJson1 carries the error shape in `__type`, and the framework also sets the
    // `x-amzn-errortype` header; fold the header in so one accessor reads either.
    if let (Some(error_type), Value::Object(map)) = (error_type, &mut parsed) {
        map.entry("__type")
            .or_insert_with(|| Value::String(error_type));
    }
    (status, parsed)
}

/// The modelled error shape a failed call reported.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        // An error `__type` may be namespaced (`com.amazonaws.ssmquicksetup#Foo`).
        .rsplit('#')
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// The `Message` a refusal carries.
fn error_message(body: &Value) -> String {
    body.get("Message")
        .and_then(Value::as_str)
        .unwrap_or("<no Message>")
        .to_string()
}

/// Percent-encode a value for use as one `@httpLabel` path segment, which is what an
/// SDK does: a non-greedy label may not carry a raw `/`.
fn label(value: &str) -> String {
    value
        .chars()
        .map(|c| match c {
            '/' => "%2F".to_string(),
            ' ' => "%20".to_string(),
            other => other.to_string(),
        })
        .collect()
}

/// A query-string value with every byte outside RFC 3986's unreserved set
/// percent-encoded, which is what an SDK writes for an `@httpQuery` member.
fn query_value(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                char::from(byte).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// `DELETE /tags/{ResourceArn}?tagKeys=…` — `UntagResource`. `TagKeys` is an
/// `@httpQuery` list, so each key travels as its own repeated `tagKeys=` parameter.
async fn untag(router: &MultiServiceRouter, resource_arn: &str, keys: &[&str]) -> (u16, Value) {
    let query: Vec<String> = keys
        .iter()
        .map(|key| format!("tagKeys={}", query_value(key)))
        .collect();
    let uri = format!("/tags/{}?{}", label(resource_arn), query.join("&"));
    call(router, "DELETE", &uri, None).await
}

/// `PUT /tags/{ResourceArn}` — `TagResource`.
async fn tag(
    router: &MultiServiceRouter,
    resource_arn: &str,
    tags: &BTreeMap<String, String>,
) -> (u16, Value) {
    let body = json!({ "Tags": tag_object(tags) });
    call(
        router,
        "PUT",
        &format!("/tags/{}", label(resource_arn)),
        Some(&body),
    )
    .await
}

/// A tag map as a restJson1 payload member.
fn tag_object(tags: &BTreeMap<String, String>) -> Value {
    let mut map = Map::new();
    for (key, value) in tags {
        map.insert(key.clone(), Value::String(value.clone()));
    }
    Value::Object(map)
}

/// `POST /configurationManager` carrying `tags` — `CreateConfigurationManager`.
async fn create(
    router: &MultiServiceRouter,
    name: &str,
    tags: &BTreeMap<String, String>,
) -> (u16, Value) {
    // `Type` must name a configuration type this service publishes; the catalog's first
    // entry is the one every case here deploys, since no case is about the type.
    let body = json!({
        "Name": name,
        "ConfigurationDefinitions": [{
            "Type": QUICK_SETUP_TYPE_CATALOG[0].type_,
            "Parameters": {},
        }],
        "Tags": tag_object(tags),
    });
    call(router, "POST", "/configurationManager", Some(&body)).await
}

/// Create one configuration manager carrying `tags`, answering its ARN.
async fn create_ok(
    router: &MultiServiceRouter,
    case: &str,
    tags: &BTreeMap<String, String>,
) -> String {
    let (status, answer) = create(router, "reserved-tag-case", tags).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateConfigurationManager must succeed, got {status} / {answer:?}"
    );
    answer
        .get("ManagerArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a ManagerArn, got {answer:?}"))
        .to_string()
}

/// The tags `GetConfigurationManager` reports for `manager_arn`.
async fn stored_tags(
    router: &MultiServiceRouter,
    case: &str,
    manager_arn: &str,
) -> BTreeMap<String, String> {
    let (status, answer) = call(
        router,
        "GET",
        &format!("/configurationManager/{}", label(manager_arn)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetConfigurationManager must succeed, got {status} / {answer:?}"
    );
    let tags = answer
        .get("Tags")
        .unwrap_or_else(|| panic!("case {case:?}: expected a Tags member, got {answer:?}"));
    tags.as_object()
        .unwrap_or_else(|| panic!("case {case:?}: Tags must be a map, got {tags:?}"))
        .iter()
        .map(|(key, value)| {
            (
                key.clone(),
                value
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: tag {key:?} must be a string"))
                    .to_string(),
            )
        })
        .collect()
}

/// The tags `ListTagsForResource` reports for `manager_arn` — the OTHER reader a stored
/// tag becomes visible through, so a refused write is shown to have left neither reader
/// reporting a reserved key. The entries are parsed by the one helper that owns that
/// reader, `all/list_tags_for_resource.rs`'s `listed`.
async fn listed_tags(
    router: &MultiServiceRouter,
    case: &str,
    manager_arn: &str,
) -> BTreeMap<String, String> {
    super::list_tags_for_resource::listed(router, case, manager_arn).await
}

/// How many configuration managers the store holds.
async fn manager_count(router: &MultiServiceRouter, case: &str) -> usize {
    let (status, answer) = call(
        router,
        "POST",
        "/listConfigurationManagers",
        Some(&json!({})),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListConfigurationManagers must succeed, got {status} / {answer:?}"
    );
    answer
        .get("ConfigurationManagersList")
        .and_then(Value::as_array)
        .map_or(0, Vec::len)
}

/// Tags as the tables below write them.
fn tags(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// The ordinary tags every case seeds its resource with, so a refusal can be shown to
/// have left the stored set alone.
fn seeded() -> BTreeMap<String, String> {
    tags(&[("Env", "prod"), ("Owner", "ops")])
}

/// Keys the rule REFUSES, spanning the casing of the reserved prefix and what follows
/// it. Each is derived from [`RESERVED_TAG_PREFIX`] rather than written out, so the
/// table follows the handler-side constant.
///
/// The casings come from the AWS General Reference's tag naming limits, which reserve
/// the prefix in any combination of upper and lower case; the tails span nothing at all,
/// a plain word, one of the punctuation characters `TagsMap`'s key pattern admits
/// (`smithy_query(query="shape",shape="TagsMap")`) and a multi-byte tail, which only
/// `UntagResource$TagKeys` (a bare `list<String>`) can carry.
fn reserved_keys() -> Vec<(String, String)> {
    let prefix = RESERVED_TAG_PREFIX;
    let upper = prefix.to_ascii_uppercase();
    // The prefix with only its first letter upper-cased, derived from the prefix itself.
    let mut mixed = String::new();
    for (index, character) in prefix.chars().enumerate() {
        if index == 0 {
            mixed.extend(character.to_uppercase());
        } else {
            mixed.push(character);
        }
    }
    vec![
        ("the reserved prefix alone".to_string(), prefix.to_string()),
        (
            "the reserved prefix with a plain tail".to_string(),
            format!("{prefix}createdBy"),
        ),
        (
            "the reserved prefix with a punctuated tail".to_string(),
            format!("{prefix}cloudformation:stack-name"),
        ),
        (
            "the reserved prefix with a multi-byte tail".to_string(),
            format!("{prefix}владелец"),
        ),
        (
            "the reserved prefix in upper case".to_string(),
            format!("{upper}createdBy"),
        ),
        (
            "the reserved prefix in mixed case".to_string(),
            format!("{mixed}CreatedBy"),
        ),
        (
            "the reserved prefix in upper case, alone".to_string(),
            upper.clone(),
        ),
    ]
}

/// Keys that only RESEMBLE the reserved prefix, so they are the caller's own: the
/// prefix's letters without its separator, the prefix one character short, the prefix
/// somewhere other than the key's start, and the prefix's letters followed by more
/// letters.
fn lookalike_keys() -> Vec<(String, String)> {
    let prefix = RESERVED_TAG_PREFIX;
    let letters: String = prefix
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .collect();
    vec![
        (
            "the prefix's letters without its separator".to_string(),
            letters.clone(),
        ),
        (
            "the prefix's letters followed by more letters".to_string(),
            format!("{letters}ome"),
        ),
        (
            "the prefix one character short of itself".to_string(),
            format!("{}.region", &prefix[..prefix.len() - 1]),
        ),
        (
            "the reserved prefix away from the key's start".to_string(),
            format!("team.{prefix}owner"),
        ),
        (
            "the prefix's letters joined to a tail by another separator".to_string(),
            format!("{letters}_createdBy"),
        ),
    ]
}

/// Every refusal states the member, the reserved prefix and the offending key.
fn assert_reserved_refusal(case: &str, member: &str, key: &str, status: u16, answer: &Value) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected 400, got {status} / {answer:?}"
    );
    assert_eq!(
        error_code(answer),
        "ValidationException",
        "case {case:?}: expected ValidationException, got {answer:?}"
    );
    let message = error_message(answer);
    assert!(
        message.contains(member),
        "case {case:?}: the refusal must name the member {member}, got {message:?}"
    );
    assert!(
        message.contains(RESERVED_TAG_PREFIX),
        "case {case:?}: the refusal must name the reserved prefix {RESERVED_TAG_PREFIX:?}, \
         got {message:?}"
    );
    assert!(
        message.contains(key),
        "case {case:?}: the refusal must name the key {key:?}, got {message:?}"
    );
}

/// `UntagResource` refuses a reserved key and removes nothing — alone, beside a key the
/// resource carries, and beside a key it does not.
#[tokio::test]
async fn untag_resource_refuses_a_reserved_tag_key() {
    let router = emulator();
    let stored = seeded();

    for (case, key) in reserved_keys() {
        let arn = create_ok(&router, &case, &stored).await;
        // The request shapes differ in how many keys they name and in what the other
        // keys are: the reserved key alone, with a stored key, and with an absent one
        // (whose removal would otherwise be a no-op success).
        for (shape, keys) in [
            ("alone", vec![key.as_str()]),
            ("beside a stored key", vec!["Env", key.as_str()]),
            ("beside an absent key", vec![key.as_str(), "Absent"]),
        ] {
            let case = format!("{case} {shape}");
            let (status, answer) = untag(&router, &arn, &keys).await;
            assert_reserved_refusal(&case, "TagKeys", &key, status, &answer);

            let left = stored_tags(&router, &case, &arn).await;
            assert_eq!(
                left, stored,
                "case {case:?}: a refused removal must remove nothing, expected {stored:?}, \
                 got {left:?}"
            );
        }
    }
}

/// A reserved key is refused as a REQUEST, before the ARN is resolved: the modelled 400
/// answers even when the ARN names no stored resource, which would otherwise be a 404.
#[tokio::test]
async fn untag_resource_refuses_a_reserved_tag_key_before_resolving_the_resource() {
    let router = emulator();

    // The 404 the same ARN earns with an ordinary key, so the rows differ only in
    // whether the key is reserved.
    let (status, answer) = untag(&router, UNKNOWN_MANAGER_ARN, &["Env"]).await;
    assert_eq!(
        status, 404,
        "case \"an ordinary key on an unknown ARN\": expected 404, got {status} / {answer:?}"
    );

    for (case, key) in reserved_keys() {
        let case = format!("{case} on an unknown ARN");
        let (status, answer) = untag(&router, UNKNOWN_MANAGER_ARN, &[key.as_str()]).await;
        assert_reserved_refusal(&case, "TagKeys", &key, status, &answer);
    }
}

/// A key that only resembles the reserved prefix is the caller's own: it is stored,
/// reported and removed like any other key.
#[tokio::test]
async fn untag_resource_removes_a_key_that_only_resembles_the_reserved_prefix() {
    let router = emulator();

    for (case, key) in lookalike_keys() {
        let mut created = seeded();
        created.insert(key.clone(), "mine".to_string());
        let arn = create_ok(&router, &case, &created).await;

        let (status, answer) = untag(&router, &arn, &[key.as_str()]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: removing an ordinary key must succeed, got {status} / {answer:?}"
        );
        let left = stored_tags(&router, &case, &arn).await;
        assert_eq!(
            left,
            seeded(),
            "case {case:?}: exactly the named key must be gone, expected {:?}, got {left:?}",
            seeded()
        );
    }
}

/// `TagResource` refuses a reserved key and stores nothing from that request.
#[tokio::test]
async fn tag_resource_refuses_a_reserved_tag_key() {
    let router = emulator();
    let stored = seeded();

    for (case, key) in reserved_keys() {
        // A map key travels in the restJson1 payload, so `TagsMap`'s key `@pattern`
        // applies: a multi-byte key is refused by the constraint walk before the rule
        // under test is reached, and belongs to the `UntagResource` table only.
        if !key.is_ascii() {
            continue;
        }
        let arn = create_ok(&router, &case, &stored).await;
        for (shape, assign) in [
            ("alone", tags(&[(key.as_str(), "mine")])),
            (
                "beside an ordinary pair",
                tags(&[(key.as_str(), "mine"), ("Tier", "gold")]),
            ),
        ] {
            let case = format!("{case} {shape}");
            let (status, answer) = tag(&router, &arn, &assign).await;
            assert_reserved_refusal(&case, "Tags", &key, status, &answer);

            let left = stored_tags(&router, &case, &arn).await;
            assert_eq!(
                left, stored,
                "case {case:?}: a refused assignment must store nothing, expected {stored:?}, \
                 got {left:?}"
            );
        }
    }
}

/// The OTHER reader agrees: after a refused assignment `ListTagsForResource` reports the
/// stored set unchanged, so no reserved key becomes visible through the tagging reader
/// even though the write path is what would have stored it.
#[tokio::test]
async fn list_tags_for_resource_reports_no_reserved_key_after_a_refused_assignment() {
    let router = emulator();
    let stored = seeded();

    for (case, key) in reserved_keys() {
        // As in `tag_resource_refuses_a_reserved_tag_key`: a body map key is bound by
        // `TagsMap`'s key `@pattern`, so a multi-byte key never reaches the rule.
        if !key.is_ascii() {
            continue;
        }
        let arn = create_ok(&router, &case, &stored).await;

        let (status, answer) = tag(&router, &arn, &tags(&[(key.as_str(), "mine")])).await;
        assert_reserved_refusal(&case, "Tags", &key, status, &answer);

        let listed = listed_tags(&router, &case, &arn).await;
        assert_eq!(
            listed, stored,
            "case {case:?}: ListTagsForResource must report the stored set, expected \
             {stored:?}, got {listed:?}"
        );
    }
}

/// A key that only resembles the reserved prefix is assigned and read back.
#[tokio::test]
async fn tag_resource_assigns_a_key_that_only_resembles_the_reserved_prefix() {
    let router = emulator();

    for (case, key) in lookalike_keys() {
        let arn = create_ok(&router, &case, &seeded()).await;
        let assign = tags(&[(key.as_str(), "mine")]);
        let (status, answer) = tag(&router, &arn, &assign).await;
        assert_eq!(
            status, 200,
            "case {case:?}: assigning an ordinary key must succeed, got {status} / {answer:?}"
        );

        let mut expected = seeded();
        expected.insert(key.clone(), "mine".to_string());
        let left = stored_tags(&router, &case, &arn).await;
        assert_eq!(
            left, expected,
            "case {case:?}: the assigned pair must be stored, expected {expected:?}, \
             got {left:?}"
        );
    }
}

/// `CreateConfigurationManager` refuses a reserved key in `Tags` and creates nothing, so
/// a reserved tag can never enter the store in the first place.
#[tokio::test]
async fn create_configuration_manager_refuses_a_reserved_tag_key() {
    let router = emulator();

    for (case, key) in reserved_keys() {
        // As in `tag_resource_refuses_a_reserved_tag_key`: a body map key is bound by
        // `TagsMap`'s key `@pattern`, so a multi-byte key never reaches the rule.
        if !key.is_ascii() {
            continue;
        }
        let before = manager_count(&router, &case).await;
        for (shape, supplied) in [
            ("alone", tags(&[(key.as_str(), "mine")])),
            (
                "beside an ordinary pair",
                tags(&[(key.as_str(), "mine"), ("Env", "prod")]),
            ),
        ] {
            let case = format!("{case} {shape}");
            let (status, answer) = create(&router, "reserved-tag-create", &supplied).await;
            assert_reserved_refusal(&case, "Tags", &key, status, &answer);

            let after = manager_count(&router, &case).await;
            assert_eq!(
                after, before,
                "case {case:?}: a refused create must store no manager, expected {before}, \
                 got {after}"
            );
        }
    }
}

/// A create carrying only lookalike keys succeeds and reports them back.
#[tokio::test]
async fn create_configuration_manager_stores_keys_that_only_resemble_the_reserved_prefix() {
    let router = emulator();

    let supplied: BTreeMap<String, String> = lookalike_keys()
        .into_iter()
        .map(|(_, key)| (key, "mine".to_string()))
        .collect();
    let case = "every lookalike key in one create";
    let arn = create_ok(&router, case, &supplied).await;
    let stored = stored_tags(&router, case, &arn).await;
    assert_eq!(
        stored, supplied,
        "case {case:?}: every ordinary key must be stored, expected {supplied:?}, \
         got {stored:?}"
    );
}
