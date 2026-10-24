// SPDX-License-Identifier: CC0-1.0

//! AWS's reserved tag-key namespace, over the wire.
//!
//! The rule under test: a tag key that claims the reserved `aws:` prefix (however the
//! prefix is cased) belongs to Amazon Web Services and not to the caller — so every
//! operation of this service that takes tag keys from the request refuses one, with the
//! refusal that operation models, and the stored tags are left exactly as they were.
//! That is `TagResource` and `CreateEnvironment`, which write keys, and
//! `UntagResource`, which deletes them: a caller can no more delete a tag Amazon Web
//! Services applies than create one.
//!
//! The reservation is a SERVICE rule rather than a shape constraint: the modelled
//! `TagKey` is `^[\w.:/=+-@]+$` with length 1..128
//! (`smithy_query(query="shape",shape="TagKey",model_path="services/service-com-amazonaws-evs-amazonelasticvmwareservice/smithy-model/com-amazonaws-evs-amazonelasticvmwareservice.json")`),
//! which admits a colon, so the boundary validator passes every key below and what
//! refuses the reserved ones is the handler.
//!
//! Which refusal each operation answers with comes from its modelled error set
//! (`smithy_query(query="operation",operation="TagResource,UntagResource,CreateEnvironment")`):
//! `TagResource` and `UntagResource` declare `TagPolicyException` (HTTP 400) — the
//! tag-policy refusal — while `CreateEnvironment` declares only `ValidationException`,
//! so a reserved `tags` key is one more entry of its aggregated field refusal.
//!
//! The refused rows differ along the casing of the prefix, what follows it (nothing, one
//! character, a real AWS system key, the longest key the modelled length allows) and
//! how many reserved keys one request carries; the accepted rows are keys that merely
//! RESEMBLE the prefix, so what is asserted is a prefix test and not a substring one.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_evs_amazonelasticvmwareservice::state::{
    is_reserved_tag_key, RESERVED_TAG_KEY_PREFIX,
};

use crate::environment_lifecycle::{call, emulator, error_type, Emulator};

/// The longest key the modelled `TagKey` admits (len 1..128), inside the reserved
/// namespace: the prefix plus enough characters to reach that bound exactly.
fn longest_reserved_key() -> String {
    let mut key = RESERVED_TAG_KEY_PREFIX.to_string();
    key.push_str(&"a".repeat(128 - RESERVED_TAG_KEY_PREFIX.len()));
    key
}

/// Tag keys that claim the reserved namespace, each with the case naming it.
///
/// The prefix itself is read from the handler-side constant rather than re-spelled
/// here, so the rows and the rule cannot drift apart.
fn reserved_keys() -> Vec<(String, String)> {
    let prefix = RESERVED_TAG_KEY_PREFIX;
    vec![
        ("the bare prefix".to_string(), prefix.to_string()),
        (
            "a real AWS system key".to_string(),
            format!("{prefix}cloudformation:stack-name"),
        ),
        (
            "one character past the prefix".to_string(),
            format!("{prefix}x"),
        ),
        (
            "the prefix in upper case".to_string(),
            format!("{}owner", prefix.to_uppercase()),
        ),
        (
            "the prefix in mixed case".to_string(),
            "Aws:Created-By".to_string(),
        ),
        (
            "the longest key the model allows".to_string(),
            longest_reserved_key(),
        ),
    ]
}

/// Tag keys that only RESEMBLE the reserved namespace: the rule is a test on the key's
/// FIRST characters, so a key carrying `aws` elsewhere, or without the colon, is an
/// ordinary customer key and is stored.
fn accepted_keys() -> Vec<(&'static str, &'static str)> {
    vec![
        ("the prefix letters without the colon", "aws"),
        ("a longer word opening with those letters", "awsome:team"),
        ("the prefix away from the start", "team:aws:cost"),
        ("a dot where the prefix has a colon", "aws.cost-center"),
        ("an underscore before the prefix", "_aws:cost"),
        ("upper-case letters and no colon", "AWSResources"),
    ]
}

/// A JSON tag map from key/value pairs, so a key can be an expression.
fn tag_map(entries: &[(&str, &str)]) -> Value {
    let mut map = serde_json::Map::new();
    for (key, value) in entries {
        map.insert((*key).to_string(), json!(value));
    }
    Value::Object(map)
}

/// The tags a test environment starts with, so a refusal can be shown to have changed
/// nothing rather than to have landed on an empty map.
fn starting_tags() -> Value {
    tag_map(&[("team", "platform"), ("cost", "42")])
}

/// Create an environment carrying `tags` and answer the raw status and body.
async fn create_with_tags(router: &Emulator, name: &str, tags: Option<Value>) -> (u16, Value) {
    let mut request = router.create_request(Some(name));
    if let Some(tags) = tags {
        request["tags"] = tags;
    }
    call(router, "CreateEnvironment", &request).await
}

/// Create an environment carrying [`starting_tags`] and answer its ARN.
async fn tagged_environment(router: &Emulator, name: &str) -> String {
    let (status, body) = create_with_tags(router, name, Some(starting_tags())).await;
    assert_eq!(
        status, 200,
        "case create {name:?}: expected HTTP 200, got {status} {body}"
    );
    body["environment"]["environmentArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case create {name:?}: no environmentArn in {body}"))
        .to_string()
}

/// The tags the service reports for `resource_arn`, read back through
/// `ListTagsForResource`.
async fn stored_tags(router: &Emulator, case: &str, resource_arn: &str) -> Value {
    let (status, body) = call(
        router,
        "ListTagsForResource",
        &json!({ "resourceArn": resource_arn }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: reading the tags back: expected HTTP 200, got {status} {body}"
    );
    body["tags"].clone()
}

/// Assert one tag-policy refusal: HTTP 400, a `TagPolicyException`, and a message built
/// from the request — it names the offending key and the prefix that is reserved.
fn assert_tag_policy_refusal(case: &str, status: u16, body: &Value, key: &str) {
    assert_eq!(
        status,
        400,
        "case {case:?}: expected HTTP 400, got {status} ({}) {body}",
        error_type(body)
    );
    assert_eq!(
        error_type(body),
        "TagPolicyException",
        "case {case:?}: expected a TagPolicyException, got {}",
        error_type(body)
    );
    let message = body["message"].as_str().unwrap_or_default();
    assert!(
        message.contains(key),
        "case {case:?}: the refusal must name the offending key {key:?}, got {message:?}"
    );
    assert!(
        message.contains(RESERVED_TAG_KEY_PREFIX),
        "case {case:?}: the refusal must name the reserved prefix \
         {RESERVED_TAG_KEY_PREFIX:?}, got {message:?}"
    );
}

#[tokio::test]
async fn tag_resource_refuses_a_reserved_key_and_writes_nothing() {
    let router = emulator();
    let arn = tagged_environment(&router, "reserved-tag").await;

    for (why, key) in reserved_keys() {
        let case = format!("TagResource with {why}");
        let (status, body) = call(
            &router,
            "TagResource",
            &json!({
                "resourceArn": arn,
                "tags": tag_map(&[(key.as_str(), "x")]),
            }),
        )
        .await;
        assert_tag_policy_refusal(&case, status, &body, &key);

        // Nothing was written: the stored tags are still the ones the create set, and
        // the refused key is not among them.
        let tags = stored_tags(&router, &case, &arn).await;
        assert_eq!(
            tags,
            starting_tags(),
            "case {case:?}: a refused TagResource must leave the stored tags unchanged, \
             got {tags}"
        );
    }

    // A request carrying a reserved key BESIDE acceptable ones is refused whole: the
    // acceptable keys are not written either.
    let case = "TagResource with a reserved key beside acceptable ones";
    let reserved = format!("{RESERVED_TAG_KEY_PREFIX}owner");
    let (status, body) = call(
        &router,
        "TagResource",
        &json!({
            "resourceArn": arn,
            "tags": tag_map(&[
                ("owner", "platform"),
                (reserved.as_str(), "aws"),
                ("stage", "test"),
            ]),
        }),
    )
    .await;
    assert_tag_policy_refusal(case, status, &body, &reserved);
    let tags = stored_tags(&router, case, &arn).await;
    assert_eq!(
        tags,
        starting_tags(),
        "case {case:?}: a request refused for one key must write none of them, got {tags}"
    );
}

#[tokio::test]
async fn tag_resource_stores_a_key_that_only_resembles_the_reserved_prefix() {
    let router = emulator();
    let arn = tagged_environment(&router, "lookalike-tag").await;

    for (why, key) in accepted_keys() {
        let case = format!("TagResource with {why}");
        let (status, body) = call(
            &router,
            "TagResource",
            &json!({
                "resourceArn": arn,
                "tags": tag_map(&[(key, "stored")]),
            }),
        )
        .await;
        assert_eq!(
            status,
            200,
            "case {case:?}: expected HTTP 200, got {status} ({}) {body}",
            error_type(&body)
        );

        // Round trip: the key the write accepted is the key a read reports.
        let tags = stored_tags(&router, &case, &arn).await;
        assert_eq!(
            tags[key],
            json!("stored"),
            "case {case:?}: expected the key {key:?} to be stored, got {tags}"
        );
    }
}

#[tokio::test]
async fn untag_resource_refuses_a_reserved_key_and_removes_nothing() {
    let router = emulator();
    let arn = tagged_environment(&router, "reserved-untag").await;

    for (why, key) in reserved_keys() {
        let case = format!("UntagResource with {why}");
        // The reserved key travels beside a key the resource really carries, so a
        // refusal that removed the rest would be visible.
        let (status, body) = call(
            &router,
            "UntagResource",
            &json!({
                "resourceArn": arn,
                "tagKeys": [key.as_str(), "team"],
            }),
        )
        .await;
        assert_tag_policy_refusal(&case, status, &body, &key);

        let tags = stored_tags(&router, &case, &arn).await;
        assert_eq!(
            tags,
            starting_tags(),
            "case {case:?}: a refused UntagResource must remove nothing, got {tags}"
        );
    }

    // The refusals above are the namespace rule's answer and not a blanket one: the
    // same operation removes an ordinary key.
    let case = "UntagResource with an ordinary key";
    let (status, body) = call(
        &router,
        "UntagResource",
        &json!({ "resourceArn": arn, "tagKeys": ["team"] }),
    )
    .await;
    assert_eq!(
        status,
        200,
        "case {case:?}: expected HTTP 200, got {status} ({}) {body}",
        error_type(&body)
    );
    let tags = stored_tags(&router, case, &arn).await;
    assert_eq!(
        tags,
        tag_map(&[("cost", "42")]),
        "case {case:?}: expected only the untouched key to remain, got {tags}"
    );
}

#[tokio::test]
async fn create_environment_refuses_a_reserved_tags_key_and_mints_nothing() {
    let router = emulator();
    // One environment that exists, so the count below is not a count of an empty store.
    let _ = tagged_environment(&router, "reserved-create-baseline").await;
    let before = environment_count(&router, "baseline").await;

    for (index, (why, key)) in reserved_keys().into_iter().enumerate() {
        let case = format!("CreateEnvironment with {why}");
        let (status, body) = create_with_tags(
            &router,
            &format!("reserved-create-{index}"),
            Some(tag_map(&[(key.as_str(), "x")])),
        )
        .await;
        assert_eq!(
            status,
            400,
            "case {case:?}: expected HTTP 400, got {status} ({}) {body}",
            error_type(&body)
        );
        assert!(
            error_type(&body).contains("ValidationException"),
            "case {case:?}: expected a ValidationException — the only client-input error \
             this operation models — got {}",
            error_type(&body)
        );
        let names: BTreeSet<&str> = body["fieldList"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no fieldList in {body}"))
            .iter()
            .filter_map(|entry| entry["name"].as_str())
            .collect();
        assert_eq!(
            names,
            BTreeSet::from(["tags"]),
            "case {case:?}: expected the refusal to name the tags member, got {names:?} in \
             {body}"
        );
        let message = body["message"].as_str().unwrap_or_default();
        assert!(
            message.contains(&key) && message.contains(RESERVED_TAG_KEY_PREFIX),
            "case {case:?}: the refusal must name the key {key:?} and the reserved prefix \
             {RESERVED_TAG_KEY_PREFIX:?}, got {message:?}"
        );
        assert_eq!(
            environment_count(&router, &case).await,
            before,
            "case {case:?}: a refused create must mint no environment"
        );
    }

    // A create carrying only acceptable keys stores them, and a read reports them back.
    for (index, (why, key)) in accepted_keys().into_iter().enumerate() {
        let case = format!("CreateEnvironment with {why}");
        let (status, body) = create_with_tags(
            &router,
            &format!("accepted-create-{index}"),
            Some(tag_map(&[(key, "stored")])),
        )
        .await;
        assert_eq!(
            status,
            200,
            "case {case:?}: expected HTTP 200, got {status} ({}) {body}",
            error_type(&body)
        );
        let arn = body["environment"]["environmentArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no environmentArn in {body}"))
            .to_string();
        let tags = stored_tags(&router, &case, &arn).await;
        assert_eq!(
            tags[key],
            json!("stored"),
            "case {case:?}: expected the create's tag {key:?} to be stored, got {tags}"
        );
    }
}

/// How many environments the service holds, so a refusal can be shown to have created
/// nothing.
async fn environment_count(router: &Emulator, case: &str) -> usize {
    let (status, body) = call(router, "ListEnvironments", &json!({})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: listing environments: expected HTTP 200, got {status} {body}"
    );
    body["environmentSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no environmentSummaries in {body}"))
        .len()
}

#[test]
fn the_reserved_namespace_is_a_prefix_test_on_the_keys_first_characters() {
    // The predicate the handlers share, over the rows the wire tests use plus keys a
    // request cannot carry (the modelled pattern refuses non-ASCII), including ones
    // whose fourth BYTE falls inside a multi-byte character: a prefix test on bytes
    // must answer those without panicking.
    for (why, key) in reserved_keys() {
        assert!(
            is_reserved_tag_key(&key),
            "case {why:?}: expected {key:?} to claim the reserved namespace, got false"
        );
    }
    let ordinary = accepted_keys()
        .into_iter()
        .map(|(why, key)| (why.to_string(), key.to_string()))
        .chain([
            ("a key shorter than the prefix".to_string(), "aw".to_string()),
            ("the empty key".to_string(), String::new()),
            (
                "a multi-byte character straddling the prefix".to_string(),
                "awś:x".to_string(),
            ),
            (
                "a multi-byte character at the start".to_string(),
                "日本語:x".to_string(),
            ),
        ])
        .collect::<Vec<_>>();
    for (why, key) in ordinary {
        assert!(
            !is_reserved_tag_key(&key),
            "case {why:?}: expected {key:?} to be an ordinary key, got true"
        );
    }
}
