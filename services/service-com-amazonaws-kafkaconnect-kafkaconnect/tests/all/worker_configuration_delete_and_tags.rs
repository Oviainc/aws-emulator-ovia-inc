// SPDX-License-Identifier: CC0-1.0

//! `DeleteWorkerConfiguration`, `TagResource` and `UntagResource`, end to end
//! over HTTP: the two rules a mutation of a worker configuration must obey.
//!
//! * A delete is a mutation of stored state: the configuration it names leaves
//!   the store, after which every by-ARN read of it answers the modelled
//!   `NotFoundException` (HTTP 404) and its name is free again.
//! * The tag set a resource carries is whatever create, `TagResource` and
//!   `UntagResource` last left on it, applied PER KEY: a key a write does not
//!   name keeps its value, and `ListTagsForResource` answers with exactly the
//!   keys that survived. Both writes are void, and AWS answers them 204 (see
//!   [`TAG_WRITE_STATUS`]); a write that would leave the resource holding more
//!   than the modelled maximum number of tags is refused outright.
//!
//! Every case drives the emulator the way a client does: a buffered `restJson1`
//! request through the same router the production binary assembles, and the
//! response body read back as JSON. Nothing reaches into the state struct, so a
//! handler that answered from the request instead of from state would fail here.
//!
//! The tables below span the classes the rules generalise over — a delete of a
//! tagged and an untagged configuration, repeated and single-shot, with and
//! without neighbours that must survive; a tag program that adds, overwrites,
//! removes one of several keys, removes a key that was never there, and empties
//! the set — rather than the single recorded case the issue cited. The modelled
//! bounds on `TagKey` (len 1..128) and `TagValue` (len 0..256) are exercised at
//! each published bound here; a length PAST one of them belongs to
//! `tag_length_bounds.rs`, which asserts the refusal.
//!
//! The sibling suites' request helpers are private to their own modules, so this
//! one carries its own copies of the few it needs.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_kafkaconnect_kafkaconnect::state::{
    MAX_TAGS_PER_RESOURCE, TRANSITION_SPAN,
};
use service_com_amazonaws_kafkaconnect_kafkaconnect::EmulatedService;

/// A SigV4 credential scope naming this service and the region every minted ARN
/// must carry. No authenticator is registered, so the signature is never
/// checked; the scope is what routes the request and names the region.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/kafkaconnect/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// The region in [`AUTHORIZATION`], which every ARN the service mints carries.
const REGION: &str = "eu-west-1";

/// The canonical emulator account id every ARN carries.
const ACCOUNT: &str = "000000000000";

/// Base64 of `key.converter=org.apache.kafka.connect.storage.StringConverter`:
/// a legal `propertiesFileContent` for every row, since no row is about the
/// properties file.
const PROPERTIES: &str =
    "a2V5LmNvbnZlcnRlcj1vcmcuYXBhY2hlLmthZmthLmNvbm5lY3Quc3RvcmFnZS5TdHJpbmdDb252ZXJ0ZXI=";

/// The status a successful `TagResource`/`UntagResource` answers: AWS answers
/// these void tag writes 204 No Content (the kafkaconnect model declares no
/// `@http` code for either, so Smithy defaults it to 200, while the recorded
/// wire responses and the sibling MSK model both say 204).
const TAG_WRITE_STATUS: u16 = 204;

/// Long enough that every change the clock owes after one call is due. The span
/// itself is the service's own constant, so this cannot drift from it.
async fn past_the_transition() {
    tokio::time::sleep(TRANSITION_SPAN + Duration::from_millis(400)).await;
}

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

/// `GET /v1/worker-configurations/{arn}` — `DescribeWorkerConfiguration`.
async fn describe(emulator: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    call(
        emulator,
        "GET",
        &format!("/v1/worker-configurations/{}", label(arn)),
        Value::Null,
    )
    .await
}

/// `DELETE /v1/worker-configurations/{arn}` — `DeleteWorkerConfiguration`.
async fn delete(emulator: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    call(
        emulator,
        "DELETE",
        &format!("/v1/worker-configurations/{}", label(arn)),
        Value::Null,
    )
    .await
}

/// `GET /v1/tags/{arn}` — `ListTagsForResource`, with the `tags` map read back
/// as a sorted map so two tag sets compare irrespective of member order.
async fn list_tags(emulator: &MultiServiceRouter, arn: &str) -> (u16, BTreeMap<String, String>) {
    let (status, body) = call(
        emulator,
        "GET",
        &format!("/v1/tags/{}", label(arn)),
        Value::Null,
    )
    .await;
    let tags = body
        .get("tags")
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        value.as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    (status, tags)
}

/// `POST /v1/tags/{arn}` — `TagResource`.
async fn tag(
    emulator: &MultiServiceRouter,
    arn: &str,
    tags: &[(&str, &str)],
) -> (u16, Value) {
    let body = json!({
        "tags": tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), json!(value)))
            .collect::<serde_json::Map<String, Value>>(),
    });
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

/// Create one worker configuration and return its ARN.
async fn create(
    emulator: &MultiServiceRouter,
    name: &str,
    tags: &[(&str, &str)],
) -> String {
    let mut body = json!({"name": name, "propertiesFileContent": PROPERTIES});
    if !tags.is_empty() {
        body["tags"] = Value::Object(
            tags.iter()
                .map(|(key, value)| ((*key).to_string(), json!(value)))
                .collect(),
        );
    }
    let (status, created) = call(emulator, "POST", "/v1/worker-configurations", body).await;
    assert_eq!(
        status, 200,
        "case {name:?}: CreateWorkerConfiguration must succeed, got {status} {created}"
    );
    text(&created, "workerConfigurationArn")
}

/// One row of the delete table.
struct DeleteCase {
    name: &'static str,
    tags: &'static [(&'static str, &'static str)],
    /// How many times the delete is called while it is still in flight: the
    /// operation is `@idempotent`, so a repeat must answer the same thing.
    deletes: usize,
}

/// The delete table: tagged and untagged configurations, a single-shot and a
/// repeated delete, and a name at the modelled minimum length
/// (`__stringMin1Max128`, len 1..128).
fn delete_cases() -> Vec<DeleteCase> {
    vec![
        DeleteCase {
            name: "wc-delete-untagged",
            tags: &[],
            deletes: 1,
        },
        DeleteCase {
            name: "wc-delete-tagged",
            tags: &[("env", "probe"), ("team", "data-eng")],
            deletes: 1,
        },
        DeleteCase {
            name: "wc-delete-repeated",
            tags: &[("env", "probe")],
            deletes: 3,
        },
        DeleteCase {
            name: "w",
            tags: &[],
            deletes: 2,
        },
    ]
}

#[tokio::test]
async fn a_deleted_worker_configuration_leaves_the_store_and_every_read_of_it_is_a_not_found() {
    let emulator = emulator();

    // A neighbour that is never deleted: the delete must take exactly one
    // configuration out of the store, not clear it.
    let survivor = create(&emulator, "wc-survivor", &[("keep", "me")]).await;

    let mut deleted = Vec::new();
    for case in delete_cases() {
        let arn = create(&emulator, case.name, case.tags).await;

        // The delete is a mutation: it answers the ARN it was given and the
        // transient DELETING state, and a repeat of it while the removal is
        // still owed answers exactly the same.
        for attempt in 1..=case.deletes {
            let (status, answered) = delete(&emulator, &arn).await;
            assert_eq!(
                status, 200,
                "case {:?} attempt {attempt}: DeleteWorkerConfiguration must succeed, got {status} {answered}",
                case.name
            );
            assert_eq!(
                text(&answered, "workerConfigurationArn"),
                arn,
                "case {:?} attempt {attempt}: the delete names the configuration it deleted, got {answered}",
                case.name
            );
            assert_eq!(
                text(&answered, "workerConfigurationState"),
                "DELETING",
                "case {:?} attempt {attempt}: a deleted configuration is DELETING, got {answered}",
                case.name
            );
        }

        // The span is non-zero, so a caller reading straight after the delete
        // still sees the transient state rather than a 404.
        let (status, described) = describe(&emulator, &arn).await;
        assert_eq!(
            status, 200,
            "case {:?}: a deleting configuration is still readable, got {status} {described}",
            case.name
        );
        assert_eq!(
            text(&described, "workerConfigurationState"),
            "DELETING",
            "case {:?}: the read must show the state the delete moved it to, got {described}",
            case.name
        );

        deleted.push((case.name, arn));
    }

    // The removal is owed to the clock, not to any call.
    past_the_transition().await;

    for (case, arn) in &deleted {
        let (status, answered) = describe(&emulator, arn).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a deleted configuration must be gone, got {status} {answered}"
        );
        assert!(
            error_code(&answered).contains("NotFoundException"),
            "case {case:?}: expected NotFoundException, got {answered}"
        );
        assert!(
            answered.get("message").and_then(Value::as_str).is_some(),
            "case {case:?}: the 404 carries a message member, got {answered}"
        );

        // Every other by-ARN operation agrees that it is gone.
        let (status, answered) = delete(&emulator, arn).await;
        assert_eq!(
            status, 404,
            "case {case:?}: deleting a configuration that is gone is a 404, got {status} {answered}"
        );
        let (status, tags) = list_tags(&emulator, arn).await;
        assert_eq!(
            status, 404,
            "case {case:?}: listing the tags of a configuration that is gone is a 404, got {status} {tags:?}"
        );
    }

    // The neighbour is untouched, tags and all.
    let (status, described) = describe(&emulator, &survivor).await;
    assert_eq!(
        status, 200,
        "case \"wc-survivor\": a configuration nobody deleted stays readable, got {status} {described}"
    );
    assert_eq!(
        text(&described, "workerConfigurationState"),
        "ACTIVE",
        "case \"wc-survivor\": a configuration nobody deleted stays ACTIVE, got {described}"
    );
    let (status, tags) = list_tags(&emulator, &survivor).await;
    assert_eq!(
        status, 200,
        "case \"wc-survivor\": its tags stay readable, got {status} {tags:?}"
    );
    assert_eq!(
        tags,
        BTreeMap::from([("keep".to_string(), "me".to_string())]),
        "case \"wc-survivor\": its tag set is untouched, got {tags:?}"
    );

    // A name a deleted configuration held is free again: the create that was a
    // 409 while it was there now succeeds with a fresh ARN.
    for (case, old_arn) in &deleted {
        let fresh = create(&emulator, case, &[]).await;
        assert_ne!(
            &fresh, old_arn,
            "case {case:?}: a reused name gets its own ARN, got a repeat of {old_arn:?}"
        );
        let (status, described) = describe(&emulator, &fresh).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the recreated configuration must read back, got {status} {described}"
        );
    }
}

/// One tag write in a program: either an attach or a removal.
#[derive(Debug)]
enum TagWrite {
    Attach(&'static [(&'static str, &'static str)]),
    Remove(&'static [&'static str]),
}

/// One row of the tag table: the tags the configuration is created with, the
/// writes applied to it in order, and the tag set `ListTagsForResource` must
/// then answer with.
struct TagCase {
    name: &'static str,
    created_with: &'static [(&'static str, &'static str)],
    writes: &'static [TagWrite],
    expected: &'static [(&'static str, &'static str)],
}

/// The tag table. The rows differ along what the rule generalises over: how many
/// keys a write names, whether a key is new, repeated or absent, and whether the
/// surviving set is everything, something or nothing.
fn tag_cases() -> Vec<TagCase> {
    vec![
        TagCase {
            name: "wc-tags-from-create",
            created_with: &[("env", "probe"), ("team", "data-eng")],
            writes: &[],
            expected: &[("env", "probe"), ("team", "data-eng")],
        },
        TagCase {
            name: "wc-tags-untag-one-of-two",
            created_with: &[("env", "probe"), ("team", "data-eng")],
            writes: &[TagWrite::Remove(&["team"])],
            expected: &[("env", "probe")],
        },
        TagCase {
            name: "wc-tags-attach-to-untagged",
            created_with: &[],
            writes: &[TagWrite::Attach(&[("owner", "platform")])],
            expected: &[("owner", "platform")],
        },
        TagCase {
            name: "wc-tags-overwrite-one-key",
            created_with: &[("a", "1"), ("b", "2"), ("c", "3")],
            writes: &[TagWrite::Attach(&[("b", "two")])],
            expected: &[("a", "1"), ("b", "two"), ("c", "3")],
        },
        TagCase {
            name: "wc-tags-remove-an-absent-key",
            created_with: &[("a", "1")],
            writes: &[TagWrite::Remove(&["never-attached"])],
            expected: &[("a", "1")],
        },
        TagCase {
            name: "wc-tags-remove-several-of-four",
            created_with: &[("a", "1"), ("b", "2"), ("c", "3"), ("d", "4")],
            writes: &[TagWrite::Remove(&["a", "c"])],
            expected: &[("b", "2"), ("d", "4")],
        },
        TagCase {
            name: "wc-tags-emptied",
            created_with: &[("only", "one")],
            writes: &[TagWrite::Remove(&["only"])],
            expected: &[],
        },
        TagCase {
            name: "wc-tags-reattached-after-removal",
            created_with: &[("k", "first")],
            writes: &[
                TagWrite::Remove(&["k"]),
                TagWrite::Attach(&[("k", "second")]),
            ],
            expected: &[("k", "second")],
        },
        TagCase {
            name: "wc-tags-interleaved-writes",
            created_with: &[("env", "probe")],
            writes: &[
                TagWrite::Attach(&[("team", "data-eng"), ("cost-centre", "4711")]),
                TagWrite::Remove(&["cost-centre", "absent"]),
                TagWrite::Attach(&[("env", "probe-2")]),
            ],
            expected: &[("env", "probe-2"), ("team", "data-eng")],
        },
        TagCase {
            // An empty VALUE is legal (`TagValue` is len 0..256), and is a
            // different answer from an absent key.
            name: "wc-tags-empty-value",
            created_with: &[("env", "")],
            writes: &[TagWrite::Attach(&[("other", "")])],
            expected: &[("env", ""), ("other", "")],
        },
        TagCase {
            // A removal naming only keys the resource does not carry, several
            // at once: nothing goes. (A request with NO `tagKeys` parameter at
            // all names no removal at all: it leaves a `@required` member
            // unbound and is the modelled `BadRequestException` naming it, which
            // `absent_required_query_parameter.rs` pins, so the empty list is
            // not a row in this table of writes that are applied.)
            name: "wc-tags-no-matching-keys-removed",
            created_with: &[("env", "probe")],
            writes: &[TagWrite::Remove(&["absent-one", "absent-two"])],
            expected: &[("env", "probe")],
        },
    ]
}

/// `expected` as the map `list_tags` hands back.
fn as_map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

#[tokio::test]
async fn tag_and_untag_apply_per_key_to_the_stored_tag_set() {
    let emulator = emulator();

    for case in tag_cases() {
        let arn = create(&emulator, case.name, case.created_with).await;

        for write in case.writes {
            let (status, answered) = match write {
                TagWrite::Attach(tags) => tag(&emulator, &arn, tags).await,
                TagWrite::Remove(keys) => untag(&emulator, &arn, keys).await,
            };
            assert_eq!(
                status, TAG_WRITE_STATUS,
                "case {:?}: the write {write:?} must answer HTTP {TAG_WRITE_STATUS}, got {status} {answered}",
                case.name
            );
        }

        let (status, tags) = list_tags(&emulator, &arn).await;
        assert_eq!(
            status, 200,
            "case {:?}: ListTagsForResource must succeed, got {status} {tags:?}",
            case.name
        );
        let expected = as_map(case.expected);
        assert_eq!(
            tags, expected,
            "case {:?}: expected the tag set {expected:?}, got {tags:?}",
            case.name
        );

        // A read must not change what the next caller sees.
        let (_, again) = list_tags(&emulator, &arn).await;
        assert_eq!(
            again, tags,
            "case {:?}: a second read answers what the first did, got {again:?}",
            case.name
        );

        // The resource itself is untouched by a tag write: the round trip
        // through Describe still answers the stored configuration.
        let (status, described) = describe(&emulator, &arn).await;
        assert_eq!(
            status, 200,
            "case {:?}: a tag write leaves the configuration readable, got {status} {described}",
            case.name
        );
        assert_eq!(
            text(&described, "name"),
            case.name,
            "case {:?}: the configuration keeps its name, got {described}",
            case.name
        );
        assert_eq!(
            text(&described, "workerConfigurationState"),
            "ACTIVE",
            "case {:?}: a tag write does not move the state, got {described}",
            case.name
        );
    }
}

#[tokio::test]
async fn one_resource_tag_set_is_independent_of_another_resource() {
    // The tag map is per resource ARN, so a write against one configuration
    // must leave every other configuration's tag set exactly as it was.
    let emulator = emulator();
    let first = create(&emulator, "wc-isolation-first", &[("env", "probe")]).await;
    let second = create(&emulator, "wc-isolation-second", &[("env", "probe")]).await;

    let (status, answered) = tag(&emulator, &first, &[("env", "changed"), ("extra", "x")]).await;
    assert_eq!(
        status, TAG_WRITE_STATUS,
        "tagging the first configuration must answer HTTP {TAG_WRITE_STATUS}, got {status} {answered}"
    );
    let (status, answered) = untag(&emulator, &first, &["env"]).await;
    assert_eq!(
        status, TAG_WRITE_STATUS,
        "untagging the first configuration must answer HTTP {TAG_WRITE_STATUS}, got {status} {answered}"
    );

    let (_, first_tags) = list_tags(&emulator, &first).await;
    assert_eq!(
        first_tags,
        as_map(&[("extra", "x")]),
        "case \"wc-isolation-first\": expected only the surviving key, got {first_tags:?}"
    );
    let (_, second_tags) = list_tags(&emulator, &second).await;
    assert_eq!(
        second_tags,
        as_map(&[("env", "probe")]),
        "case \"wc-isolation-second\": a write against another resource changes nothing, got {second_tags:?}"
    );
}

#[tokio::test]
async fn a_tag_key_or_value_at_a_modelled_bound_round_trips_and_is_removed_by_its_key() {
    // The bounds come from the model, not from this test: `TagKey` is len
    // 1..128 and `TagValue` is len 0..256 (smithy: Tags -> TagKey / TagValue),
    // so each of those lengths is a legal key or value the store must hold and
    // hand back unchanged, and must remove when — and only when — it is named.
    // (A key or value PAST one of those bounds is refused, which
    // `tag_length_bounds.rs` is about; this case is about the lengths the
    // bounds admit.)
    let emulator = emulator();
    let arn = create(&emulator, "wc-tag-bounds", &[]).await;

    let key_at_min = "k".to_string();
    let key_at_max = "k".repeat(128);
    let value_at_min = String::new();
    let value_at_max = "v".repeat(256);

    let rows: Vec<(&str, String, String)> = vec![
        (
            "a key at the minimum length",
            key_at_min.clone(),
            "v".to_string(),
        ),
        (
            "a key at the maximum length",
            key_at_max.clone(),
            "v".to_string(),
        ),
        (
            "a value at the minimum length",
            "value-at-min".to_string(),
            value_at_min,
        ),
        (
            "a value at the maximum length",
            "value-at-max".to_string(),
            value_at_max,
        ),
    ];

    for (case, key, value) in &rows {
        let (status, answered) = tag(&emulator, &arn, &[(key.as_str(), value.as_str())]).await;
        assert_eq!(
            status, TAG_WRITE_STATUS,
            "case {case:?}: TagResource must accept a legal key and value, got {status} {answered}"
        );
        let (status, tags) = list_tags(&emulator, &arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListTagsForResource must succeed, got {status} {tags:?}"
        );
        assert_eq!(
            tags.get(key).map(String::as_str),
            Some(value.as_str()),
            "case {case:?}: expected the stored value {value:?} for key {key:?}, got {tags:?}"
        );
    }

    // Removing one of them takes exactly that key out, whatever its length.
    let (status, answered) = untag(&emulator, &arn, &[&key_at_max]).await;
    assert_eq!(
        status, TAG_WRITE_STATUS,
        "case \"a key at the maximum length\": the removal must succeed, got {status} {answered}"
    );
    let (status, tags) = list_tags(&emulator, &arn).await;
    assert_eq!(
        status, 200,
        "the tag set must read back, got {status} {tags:?}"
    );
    assert!(
        !tags.contains_key(&key_at_max) && tags.contains_key(&key_at_min),
        "expected only the named key gone, got {tags:?}"
    );
    assert_eq!(
        tags.len(),
        rows.len() - 1,
        "expected {} surviving keys, got {tags:?}",
        rows.len() - 1
    );
}

#[tokio::test]
async fn tagging_an_arn_the_service_never_minted_is_a_not_found() {
    let emulator = emulator();
    // Every row is a WELL-FORMED kafkaconnect ARN this store does not hold: one
    // of each resource kind the service tags, and one of a resource kind it
    // mints none of. A value that is not an ARN at all is incorrect input
    // instead, which `arn_label_refusals` pins.
    let unknown: Vec<(&str, String)> = vec![
        (
            "a worker configuration that was never created",
            format!(
                "arn:aws:kafkaconnect:{REGION}:{ACCOUNT}:worker-configuration/ghost/00000000-0000-4000-8000-000000000000"
            ),
        ),
        (
            "a custom plugin that was never created",
            format!(
                "arn:aws:kafkaconnect:{REGION}:{ACCOUNT}:custom-plugin/ghost/00000000-0000-4000-8000-000000000000"
            ),
        ),
        (
            "a connector that was never created",
            format!(
                "arn:aws:kafkaconnect:{REGION}:{ACCOUNT}:connector/ghost/00000000-0000-4000-8000-000000000000"
            ),
        ),
        ("not a resource kind this service mints", format!("arn:aws:kafkaconnect:{REGION}:{ACCOUNT}:worker-configuration")),
    ];

    for (case, arn) in unknown {
        let (status, answered) = tag(&emulator, &arn, &[("env", "probe")]).await;
        assert_eq!(
            status, 404,
            "case {case:?}: tagging an unknown ARN is a 404, got {status} {answered}"
        );
        assert!(
            error_code(&answered).contains("NotFoundException"),
            "case {case:?}: expected NotFoundException, got {answered}"
        );

        let (status, answered) = untag(&emulator, &arn, &["env"]).await;
        assert_eq!(
            status, 404,
            "case {case:?}: untagging an unknown ARN is a 404, got {status} {answered}"
        );
        assert!(
            error_code(&answered).contains("NotFoundException"),
            "case {case:?}: expected NotFoundException, got {answered}"
        );
    }
}

#[tokio::test]
async fn a_resources_stored_tag_set_is_capped_at_the_modelled_maximum() {
    // The cap is the service's own constant, so this cannot drift from it: a
    // resource may carry `MAX_TAGS_PER_RESOURCE` tags, which is the modelled
    // `Tags` length maximum (0..200). The model bounds ONE request's map, which
    // the framework refuses pre-handler; this is the bound on what REPEATED tag
    // writes may accumulate on the resource.
    let emulator = emulator();
    let arn = create(&emulator, "wc-tag-cap", &[]).await;

    let at_cap: Vec<(String, String)> = (0..MAX_TAGS_PER_RESOURCE)
        .map(|index| (format!("key-{index:03}"), format!("value-{index:03}")))
        .collect();
    let at_cap_refs: Vec<(&str, &str)> = at_cap
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();

    // At the cap: accepted, and every key reads back.
    let (status, answered) = tag(&emulator, &arn, &at_cap_refs).await;
    assert_eq!(
        status, TAG_WRITE_STATUS,
        "case \"a tag set at the cap\": expected HTTP {TAG_WRITE_STATUS}, got {status} {answered}"
    );
    let (_, tags) = list_tags(&emulator, &arn).await;
    assert_eq!(
        tags.len(),
        MAX_TAGS_PER_RESOURCE,
        "case \"a tag set at the cap\": expected {MAX_TAGS_PER_RESOURCE} stored keys, got {}",
        tags.len()
    );

    // One past the cap: refused with the modelled 429, and nothing is written.
    let (status, answered) = tag(&emulator, &arn, &[("one-too-many", "x")]).await;
    assert_eq!(
        status, 429,
        "case \"one key past the cap\": expected HTTP 429, got {status} {answered}"
    );
    assert!(
        error_code(&answered).contains("TooManyRequestsException"),
        "case \"one key past the cap\": expected TooManyRequestsException, got {answered}"
    );
    let (_, after) = list_tags(&emulator, &arn).await;
    assert_eq!(
        after, tags,
        "case \"one key past the cap\": a refused write must store nothing, got {} keys",
        after.len()
    );

    // A write that does not GROW the set is still accepted at the cap: an
    // overwrite of a key already stored.
    let (status, answered) = tag(&emulator, &arn, &[("key-000", "rewritten")]).await;
    assert_eq!(
        status, TAG_WRITE_STATUS,
        "case \"an overwrite at the cap\": expected HTTP {TAG_WRITE_STATUS}, got {status} {answered}"
    );
    let (_, after) = list_tags(&emulator, &arn).await;
    assert_eq!(
        after.get("key-000").map(String::as_str),
        Some("rewritten"),
        "case \"an overwrite at the cap\": expected the new value, got {after:?}"
    );
    assert_eq!(
        after.len(),
        MAX_TAGS_PER_RESOURCE,
        "case \"an overwrite at the cap\": the set must not grow, got {} keys",
        after.len()
    );

    // Freeing a key makes room again: the write refused above now succeeds.
    let (status, answered) = untag(&emulator, &arn, &["key-000"]).await;
    assert_eq!(
        status, TAG_WRITE_STATUS,
        "case \"freeing one key\": expected HTTP {TAG_WRITE_STATUS}, got {status} {answered}"
    );
    let (status, answered) = tag(&emulator, &arn, &[("one-too-many", "x")]).await;
    assert_eq!(
        status, TAG_WRITE_STATUS,
        "case \"one key after room was freed\": expected HTTP {TAG_WRITE_STATUS}, got {status} {answered}"
    );
    let (_, after) = list_tags(&emulator, &arn).await;
    assert_eq!(
        after.len(),
        MAX_TAGS_PER_RESOURCE,
        "case \"one key after room was freed\": expected {MAX_TAGS_PER_RESOURCE} stored keys, got {}",
        after.len()
    );
    assert_eq!(
        after.get("one-too-many").map(String::as_str),
        Some("x"),
        "case \"one key after room was freed\": the new key must be stored, got {after:?}"
    );
}
