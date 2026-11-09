// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage for Recycle Bin's tagging slice: `TagResource`,
//! `UntagResource` and `ListTagsForResource`.
//!
//! Every case drives the service through the real router (`MultiServiceRouter::dispatch`),
//! so the wire protocol, the ARN-bound `@httpLabel`, the `tagKeys` query binding, the
//! boundary validator and the handlers are exercised exactly as a signed SDK request
//! exercises them. The rules under test:
//!
//! * a tagging call addresses its retention rule by the ARN this service MINTED, and an
//!   ARN no stored rule carries is HTTP 404 `ResourceNotFoundException` naming the rule
//!   IDENTIFIER that ARN carries, on every one of the three operations;
//! * `TagResource` is an UPSERT keyed by tag key: a key the rule already carries takes
//!   the new value, a key it does not carry is added, every other key survives, and no
//!   other rule's tags move;
//! * `UntagResource` removes exactly the keys it names ("All tags that have the
//!   specified tag key are unassigned") and ignores a key the rule does not carry, so
//!   the call is idempotent;
//! * `ListTagsForResource` reports the tags STORED on that one rule — whatever their
//!   number, keys or values — and reading them changes nothing a later caller sees;
//! * a rule is held to the published quota of tags per resource, so an assignment that
//!   would leave it holding more is refused with HTTP 402.
//!
//! The tables span the class each rule covers rather than one recorded tag set: the tag
//! count (none, one, several, the quota), the key and value lengths at the bounds the
//! model declares, empty values, non-ASCII keys and values, and the URI-significant
//! characters the `TagKey`/`TagValue` pattern admits.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_rbin_amazonrecyclebin::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The emulator's canonical account id, which every ARN it emits carries.
const ACCOUNT_ID: &str = engine::EMULATOR_ACCOUNT_ID;

/// The region the test signs for; the handler reads it from the SigV4 credential scope.
const REGION: &str = "eu-west-2";

/// `TagKey` is `@length(min: 1, max: 128)`.
/// Source: smithy_query(query="shape", shape="Tag") — member `Key`.
const MAX_TAG_KEY_LEN: usize = 128;

/// `TagValue` is `@length(min: 0, max: 256)`.
/// Source: smithy_query(query="shape", shape="Tag") — member `Value`.
const MAX_TAG_VALUE_LEN: usize = 256;

/// The published quota of tags per resource the store holds a rule to.
/// Source: `service_com_amazonaws_rbin_amazonrecyclebin::state::MAX_TAGS_PER_RULE`.
const MAX_TAGS_PER_RULE: usize =
    service_com_amazonaws_rbin_amazonrecyclebin::state::MAX_TAGS_PER_RULE;

fn router() -> MultiServiceRouter {
    // The single shared engine the production router builds and hands to every
    // operation of this service.
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(engine));
    // Pin authentication off so the suite does not depend on the process-wide switch.
    router.set_request_authentication(false);
    router
}

/// A SigV4 `Authorization` header whose credential scope names `rbin` in [`REGION`] —
/// the signal both the router's `can_handle` and the ARN's region segment read.
fn authorization() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDRBINTEST/20260101/{REGION}/rbin/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=deadbeef"
    )
}

/// Percent-encode a value for one URI path segment or one query value: every byte
/// outside the RFC 3986 unreserved set is escaped, which is what an SDK sends for an
/// ARN bound to a NON-greedy `@httpLabel` (`/tags/{ResourceArn}`) and for a `tagKeys`
/// query value that may hold a space, `+`, `/`, `:` or `=`.
///
/// Written out rather than reached for from `percent_encoding`, for the reason
/// `services/service-com-amazonaws-pcaconnectorscep-pcaconnectorscep/tests/all/connector_lifecycle.rs`
/// documents for its own copy of this helper: a test-only encoder is not worth a new
/// dev-dependency. `pub(crate)` so the sibling suites of this one test target address a
/// rule by ARN through the same encoder.
pub(crate) fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(*byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// Dispatch one request through the router and return its status and parsed JSON body
/// (`Null` for an empty body, as a 204 has).
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = match &body {
        Some(value) => bytes::Bytes::from(serde_json::to_vec(value).expect("serializable body")),
        None => bytes::Bytes::new(),
    };
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("https://rbin.{REGION}.amazonaws.com{path}"))
        .header("authorization", authorization());
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder.body(payload).expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            panic!(
                "{method} {path} answered {status} with a non-JSON body ({e}): {}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, parsed)
}

async fn create_rule(router: &MultiServiceRouter, body: Value) -> (u16, Value) {
    call(router, "POST", "/rules", Some(body)).await
}

async fn get_rule(router: &MultiServiceRouter, identifier: &str) -> (u16, Value) {
    call(router, "GET", &format!("/rules/{identifier}"), None).await
}

async fn delete_rule(router: &MultiServiceRouter, identifier: &str) -> (u16, Value) {
    call(router, "DELETE", &format!("/rules/{identifier}"), None).await
}

/// `GET /tags/{ResourceArn}` — `ListTagsForResource`.
pub(crate) async fn list_tags(router: &MultiServiceRouter, resource_arn: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/tags/{}", percent_encode(resource_arn)),
        None,
    )
    .await
}

/// `POST /tags/{ResourceArn}` with `Tags` in the body — `TagResource`. `pub(crate)` so
/// the sibling suites of this one test target tag a rule through the same request shape.
pub(crate) async fn tag_resource(
    router: &MultiServiceRouter,
    resource_arn: &str,
    tags: &[(String, String)],
) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/tags/{}", percent_encode(resource_arn)),
        Some(json!({ "Tags": tag_list(tags) })),
    )
    .await
}

/// `DELETE /tags/{ResourceArn}?tagKeys=..&tagKeys=..` — `UntagResource`. `TagKeys` is
/// bound to `@httpQuery("tagKeys")`, so each key travels as its own query pair.
/// `pub(crate)` for the same reason as [`tag_resource`].
pub(crate) async fn untag_resource(
    router: &MultiServiceRouter,
    resource_arn: &str,
    tag_keys: &[String],
) -> (u16, Value) {
    let query = tag_keys
        .iter()
        .map(|key| format!("tagKeys={}", percent_encode(key)))
        .collect::<Vec<_>>()
        .join("&");
    // An empty `TagKeys` list serializes to NO query pairs at all, which is a bare
    // path — not a path with a dangling `?`.
    let path = if query.is_empty() {
        format!("/tags/{}", percent_encode(resource_arn))
    } else {
        format!("/tags/{}?{query}", percent_encode(resource_arn))
    };
    call(router, "DELETE", &path, None).await
}

/// The `TagList` wire form of `(key, value)` pairs.
fn tag_list(tags: &[(String, String)]) -> Value {
    Value::Array(
        tags.iter()
            .map(|(key, value)| json!({ "Key": key, "Value": value }))
            .collect(),
    )
}

/// A response's `Tags` as a key -> value map. A map, not a list: one tag per key is the
/// invariant under test, while the ORDER of a `TagList` is not something the service
/// documents.
fn tags_of(body: &Value, member: &str) -> Tags {
    let Some(list) = body.get(member).and_then(Value::as_array) else {
        return BTreeMap::new();
    };
    let mut out = BTreeMap::new();
    for tag in list {
        let key = tag
            .get("Key")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("a Tag without a string Key: {tag:?}"))
            .to_string();
        let value = tag
            .get("Value")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("a Tag without a string Value: {tag:?}"))
            .to_string();
        assert!(
            out.insert(key.clone(), value).is_none(),
            "a resource must hold at most one tag per key, but {key:?} appears twice in {list:?}"
        );
    }
    out
}

/// The wire error code of an error response (`__type`), or `"<none>"`.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<none>")
        .to_string()
}

/// A rule's tags as the assertions compare them: one value per key, since the ORDER of
/// a `TagList` is not something the service documents.
type Tags = BTreeMap<String, String>;

/// The `(key, value)` pairs one request assigns.
type TagPairs = Vec<(String, String)>;

/// One row of an upsert table: the case, the pairs it assigns, the tags it must leave.
type UpsertRow = (&'static str, TagPairs, Tags);

/// One row of a removal table: the case, the keys it unassigns, the tags it must leave.
type RemovalRow = (&'static str, Vec<String>, Tags);

/// The `(key, value)` pairs one request assigns, as [`tag_resource`] takes them.
pub(crate) fn pairs(tags: &[(&str, &str)]) -> TagPairs {
    tags.iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

fn expected(tags: &[(&str, &str)]) -> Tags {
    tags.iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// The tag keys one request unassigns, as [`untag_resource`] takes them.
pub(crate) fn keys(keys: &[&str]) -> Vec<String> {
    keys.iter().map(|key| (*key).to_string()).collect()
}

/// A minimal rule body: the smallest request `CreateRule` accepts, plus whatever tags
/// the case wants the rule to start life with.
fn rule_body(description: &str, tags: &[(String, String)]) -> Value {
    json!({
        "ResourceType": "EBS_SNAPSHOT",
        "RetentionPeriod": { "RetentionPeriodValue": 7, "RetentionPeriodUnit": "DAYS" },
        "Description": description,
        "Tags": tag_list(tags),
    })
}

/// Create a rule and return `(Identifier, RuleArn)`.
async fn rule(router: &MultiServiceRouter, description: &str) -> (String, String) {
    rule_with_tags(router, description, &[]).await
}

async fn rule_with_tags(
    router: &MultiServiceRouter,
    description: &str,
    tags: &[(String, String)],
) -> (String, String) {
    let (status, created) = create_rule(router, rule_body(description, tags)).await;
    assert_eq!(
        status, 201,
        "case {description:?}: fixture CreateRule returned {status} / {created:?}"
    );
    let identifier = created["Identifier"]
        .as_str()
        .unwrap_or_else(|| panic!("case {description:?}: CreateRule reported no Identifier"))
        .to_string();
    let rule_arn = created["RuleArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {description:?}: CreateRule reported no RuleArn"))
        .to_string();
    assert_eq!(
        rule_arn,
        format!("arn:aws:rbin:{REGION}:{ACCOUNT_ID}:rule/{identifier}"),
        "case {description:?}: expected the minted ARN for {identifier}, got {rule_arn:?}"
    );
    (identifier, rule_arn)
}

/// Tag sets spanning the class the tagging rules generalise over: the count, the key and
/// value lengths at the bounds the model declares, an empty value, non-ASCII text, and
/// the URI-significant characters `TagKey`/`TagValue`'s pattern
/// (`^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$`) admits.
fn tag_set_cases() -> Vec<(&'static str, TagPairs)> {
    let long_key = format!("key.{}", "k".repeat(MAX_TAG_KEY_LEN - 4));
    let long_value = "v".repeat(MAX_TAG_VALUE_LEN);
    vec![
        ("no tags at all", Vec::new()),
        ("one tag", pairs(&[("env", "dev")])),
        (
            "several tags",
            pairs(&[("env", "prod"), ("team", "platform"), ("cost.centre", "42")]),
        ),
        ("an empty value", pairs(&[("team", "")])),
        (
            "key and value at their maximum length",
            vec![(long_key, long_value)],
        ),
        (
            "non-ASCII key and value",
            pairs(&[("clé", "naïve 日本"), ("Ω", "Ω")]),
        ),
        (
            "URI-significant characters the pattern admits",
            pairs(&[
                ("path/to:thing", "a=b+c"),
                ("with space", "@owner"),
                ("plus+key", "slash/value"),
            ]),
        ),
        (
            "the quota's worth of tags",
            (0..MAX_TAGS_PER_RULE)
                .map(|nth| (format!("key{nth}"), format!("value{nth}")))
                .collect(),
        ),
    ]
}

#[tokio::test]
async fn tags_assigned_by_tag_resource_are_reported_by_list_tags_for_resource() {
    let router = router();
    for (case, tags) in tag_set_cases() {
        let (identifier, rule_arn) = rule(&router, case).await;

        // A rule created with no tags starts with none.
        let (status, listed) = list_tags(&router, &rule_arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListTagsForResource returned {status} / {listed:?}"
        );
        assert_eq!(
            tags_of(&listed, "Tags"),
            BTreeMap::new(),
            "case {case:?}: an untagged rule must report no tags, got {:?}",
            listed["Tags"]
        );

        let (status, body) = tag_resource(&router, &rule_arn, &tags).await;
        assert_eq!(
            status, 201,
            "case {case:?}: TagResource returned {status} / {body:?}"
        );

        let want: BTreeMap<String, String> = tags.iter().cloned().collect();
        let (status, listed) = list_tags(&router, &rule_arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListTagsForResource returned {status} / {listed:?}"
        );
        assert_eq!(
            tags_of(&listed, "Tags"),
            want,
            "case {case:?}: expected ListTagsForResource to report {want:?}, got {:?}",
            listed["Tags"]
        );

        // Reading tags must not change what the next caller sees.
        let (status, again) = list_tags(&router, &rule_arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the second ListTagsForResource returned {status} / {again:?}"
        );
        assert_eq!(
            tags_of(&again, "Tags"),
            want,
            "case {case:?}: reading tags must not change them, got {:?}",
            again["Tags"]
        );

        // Nor may tagging disturb the rest of the rule.
        let (status, fetched) = get_rule(&router, &identifier).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetRule returned {status} / {fetched:?}"
        );
        assert_eq!(
            fetched["Description"].as_str(),
            Some(case),
            "case {case:?}: tagging must not change the rule's Description, got {:?}",
            fetched["Description"]
        );
        assert_eq!(
            fetched["RetentionPeriod"]["RetentionPeriodValue"].as_i64(),
            Some(7),
            "case {case:?}: tagging must not change the rule's retention period, got {:?}",
            fetched["RetentionPeriod"]
        );
    }
}

#[tokio::test]
async fn create_rule_tags_are_the_same_stored_tags_the_tag_operations_work_on() {
    let router = router();
    for (case, tags) in tag_set_cases() {
        let (_identifier, rule_arn) = rule_with_tags(&router, case, &tags).await;
        let want: BTreeMap<String, String> = tags.iter().cloned().collect();

        let (status, listed) = list_tags(&router, &rule_arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListTagsForResource returned {status} / {listed:?}"
        );
        assert_eq!(
            tags_of(&listed, "Tags"),
            want,
            "case {case:?}: expected the tags CreateRule stored, got {:?}",
            listed["Tags"]
        );

        // And unassigning every one of them empties the same store. `TagKeys` is a
        // `@required` `@httpQuery` binding, so an EMPTY key list puts no query pair on
        // the wire and names no operation at all — a routing matter, not this handler's,
        // so the no-tag row has nothing to unassign.
        let all: Vec<String> = tags.iter().map(|(key, _)| key.clone()).collect();
        if all.is_empty() {
            continue;
        }
        let (status, body) = untag_resource(&router, &rule_arn, &all).await;
        assert_eq!(
            status, 204,
            "case {case:?}: UntagResource returned {status} / {body:?}"
        );
        let (status, listed) = list_tags(&router, &rule_arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListTagsForResource returned {status} / {listed:?}"
        );
        assert_eq!(
            tags_of(&listed, "Tags"),
            BTreeMap::new(),
            "case {case:?}: unassigning every key must leave no tags, got {:?}",
            listed["Tags"]
        );
    }
}

#[tokio::test]
async fn tag_resource_upserts_by_key_and_leaves_every_other_key_alone() {
    let router = router();
    let initial = pairs(&[("env", "dev"), ("team", "platform"), ("owner", "")]);

    // Each row assigns a different shape of change; every key the row does not name
    // must survive with its value.
    let rows: Vec<UpsertRow> = vec![
        (
            "replacing one existing key's value",
            pairs(&[("env", "prod")]),
            expected(&[("env", "prod"), ("team", "platform"), ("owner", "")]),
        ),
        (
            "re-assigning the same value is idempotent",
            pairs(&[("env", "dev")]),
            expected(&[("env", "dev"), ("team", "platform"), ("owner", "")]),
        ),
        (
            "adding a key the rule does not carry",
            pairs(&[("tier", "gold")]),
            expected(&[
                ("env", "dev"),
                ("team", "platform"),
                ("owner", ""),
                ("tier", "gold"),
            ]),
        ),
        (
            "replacing and adding in one call",
            pairs(&[("team", "storage"), ("tier", "silver")]),
            expected(&[
                ("env", "dev"),
                ("team", "storage"),
                ("owner", ""),
                ("tier", "silver"),
            ]),
        ),
        (
            "emptying a value",
            pairs(&[("env", "")]),
            expected(&[("env", ""), ("team", "platform"), ("owner", "")]),
        ),
        (
            "filling a value that was empty",
            pairs(&[("owner", "ops")]),
            expected(&[("env", "dev"), ("team", "platform"), ("owner", "ops")]),
        ),
        (
            "assigning nothing changes nothing",
            Vec::new(),
            expected(&[("env", "dev"), ("team", "platform"), ("owner", "")]),
        ),
    ];

    for (case, written, want) in rows {
        let (_identifier, rule_arn) = rule_with_tags(&router, case, &initial).await;
        // A second rule, so a write that leaked across rules cannot pass unnoticed.
        let (_other_id, other_arn) =
            rule_with_tags(&router, "bystander", &pairs(&[("env", "sandbox")])).await;

        let (status, body) = tag_resource(&router, &rule_arn, &written).await;
        assert_eq!(
            status, 201,
            "case {case:?}: TagResource returned {status} / {body:?}"
        );

        let (_status, listed) = list_tags(&router, &rule_arn).await;
        assert_eq!(
            tags_of(&listed, "Tags"),
            want,
            "case {case:?}: expected {want:?} after assigning {written:?}, got {:?}",
            listed["Tags"]
        );

        let (_status, bystander) = list_tags(&router, &other_arn).await;
        assert_eq!(
            tags_of(&bystander, "Tags"),
            expected(&[("env", "sandbox")]),
            "case {case:?}: another rule's tags must not move, got {:?}",
            bystander["Tags"]
        );
    }
}

#[tokio::test]
async fn untag_resource_removes_only_the_keys_it_names() {
    let router = router();
    let initial = pairs(&[("env", "dev"), ("team", "platform"), ("tier", "gold")]);

    let rows: Vec<RemovalRow> = vec![
        (
            "one key",
            keys(&["team"]),
            expected(&[("env", "dev"), ("tier", "gold")]),
        ),
        ("several keys", keys(&["env", "tier"]), expected(&[("team", "platform")])),
        ("every key", keys(&["env", "team", "tier"]), BTreeMap::new()),
        (
            "a key the rule does not carry",
            keys(&["absent"]),
            expected(&[("env", "dev"), ("team", "platform"), ("tier", "gold")]),
        ),
        (
            "a carried and an absent key together",
            keys(&["env", "absent"]),
            expected(&[("team", "platform"), ("tier", "gold")]),
        ),
        (
            "the same key twice",
            keys(&["tier", "tier"]),
            expected(&[("env", "dev"), ("team", "platform")]),
        ),
    ];

    for (case, removed, want) in rows {
        let (_identifier, rule_arn) = rule_with_tags(&router, case, &initial).await;
        let (_other_id, other_arn) =
            rule_with_tags(&router, "bystander", &initial).await;

        let (status, body) = untag_resource(&router, &rule_arn, &removed).await;
        assert_eq!(
            status, 204,
            "case {case:?}: UntagResource returned {status} / {body:?}"
        );

        let (_status, listed) = list_tags(&router, &rule_arn).await;
        assert_eq!(
            tags_of(&listed, "Tags"),
            want,
            "case {case:?}: expected {want:?} after unassigning {removed:?}, got {:?}",
            listed["Tags"]
        );

        // Unassigning the same keys again is a no-op success: the call is idempotent.
        let (status, body) = untag_resource(&router, &rule_arn, &removed).await;
        assert_eq!(
            status, 204,
            "case {case:?}: the repeated UntagResource returned {status} / {body:?}"
        );
        let (_status, listed) = list_tags(&router, &rule_arn).await;
        assert_eq!(
            tags_of(&listed, "Tags"),
            want,
            "case {case:?}: unassigning twice must leave {want:?}, got {:?}",
            listed["Tags"]
        );

        let (_status, bystander) = list_tags(&router, &other_arn).await;
        assert_eq!(
            tags_of(&bystander, "Tags"),
            expected(&[("env", "dev"), ("team", "platform"), ("tier", "gold")]),
            "case {case:?}: another rule's tags must not move, got {:?}",
            bystander["Tags"]
        );
    }
}

#[tokio::test]
async fn tag_resource_is_refused_past_the_published_tags_per_resource_quota() {
    let router = router();

    // The bound itself is accepted, in one request, on a rule that holds nothing.
    let at_quota: Vec<(String, String)> = (0..MAX_TAGS_PER_RULE)
        .map(|nth| (format!("key{nth}"), format!("value{nth}")))
        .collect();
    let (_identifier, rule_arn) = rule(&router, "at the quota").await;
    let (status, body) = tag_resource(&router, &rule_arn, &at_quota).await;
    assert_eq!(
        status, 201,
        "case \"the quota's worth of tags in one call\": expected HTTP 201, got {status} / {body:?}"
    );

    // One key past it is refused, and the refusal leaves the stored tags untouched.
    let (status, body) = tag_resource(
        &router,
        &rule_arn,
        &pairs(&[("one-too-many", "refused")]),
    )
    .await;
    assert_eq!(
        status, 402,
        "case \"one key past the quota\": expected HTTP 402, got {status} / {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "ServiceQuotaExceededException",
        "case \"one key past the quota\": expected ServiceQuotaExceededException, got {body:?}"
    );
    assert_eq!(
        body["Reason"].as_str(),
        Some("SERVICE_QUOTA_EXCEEDED"),
        "case \"one key past the quota\": expected Reason SERVICE_QUOTA_EXCEEDED, got {:?}",
        body["Reason"]
    );
    let (_status, listed) = list_tags(&router, &rule_arn).await;
    let want: BTreeMap<String, String> = at_quota.iter().cloned().collect();
    assert_eq!(
        tags_of(&listed, "Tags"),
        want,
        "case \"one key past the quota\": a refused assignment must store nothing, got {} tags",
        tags_of(&listed, "Tags").len()
    );

    // Replacing the value of a key the rule already holds cannot exceed the quota: the
    // bound is on the tags the rule would be left with, not on the request's size.
    let (status, body) = tag_resource(&router, &rule_arn, &pairs(&[("key0", "rewritten")])).await;
    assert_eq!(
        status, 201,
        "case \"replacing a value at the quota\": expected HTTP 201, got {status} / {body:?}"
    );
    let (_status, listed) = list_tags(&router, &rule_arn).await;
    assert_eq!(
        tags_of(&listed, "Tags").get("key0").map(String::as_str),
        Some("rewritten"),
        "case \"replacing a value at the quota\": expected the new value, got {:?}",
        listed["Tags"]
    );
    assert_eq!(
        tags_of(&listed, "Tags").len(),
        MAX_TAGS_PER_RULE,
        "case \"replacing a value at the quota\": the tag count must not change, got {}",
        tags_of(&listed, "Tags").len()
    );

    // And the bound holds across calls that each stay under it on their own.
    let (_identifier, split_arn) = rule(&router, "filled in two calls").await;
    let first: Vec<(String, String)> = at_quota[..MAX_TAGS_PER_RULE - 1].to_vec();
    let (status, body) = tag_resource(&router, &split_arn, &first).await;
    assert_eq!(
        status, 201,
        "case \"one short of the quota\": expected HTTP 201, got {status} / {body:?}"
    );
    let (status, body) = tag_resource(&router, &split_arn, &pairs(&[("last", "ok")])).await;
    assert_eq!(
        status, 201,
        "case \"the key that reaches the quota\": expected HTTP 201, got {status} / {body:?}"
    );
    let (status, body) = tag_resource(&router, &split_arn, &pairs(&[("over", "refused")])).await;
    assert_eq!(
        status, 402,
        "case \"the key past the quota in a later call\": expected HTTP 402, got {status} / {body:?}"
    );
}

#[tokio::test]
async fn create_rule_is_held_to_the_same_tags_per_resource_quota() {
    let router = router();

    // The bound itself is accepted at creation.
    let at_quota: Vec<(String, String)> = (0..MAX_TAGS_PER_RULE)
        .map(|nth| (format!("key{nth}"), format!("value{nth}")))
        .collect();
    let (_identifier, rule_arn) = rule_with_tags(&router, "created at the quota", &at_quota).await;
    let (_status, listed) = list_tags(&router, &rule_arn).await;
    assert_eq!(
        tags_of(&listed, "Tags").len(),
        MAX_TAGS_PER_RULE,
        "case \"created at the quota\": expected {MAX_TAGS_PER_RULE} stored tags, got {:?}",
        listed["Tags"]
    );

    // One key past it is refused with the error CreateRule models for a quota, and
    // nothing is created.
    let mut past_quota = at_quota.clone();
    past_quota.push(("one-too-many".to_string(), "refused".to_string()));
    let (status, body) = create_rule(&router, rule_body("past the quota", &past_quota)).await;
    assert_eq!(
        status, 402,
        "case \"created past the quota\": expected HTTP 402, got {status} / {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "ServiceQuotaExceededException",
        "case \"created past the quota\": expected ServiceQuotaExceededException, got {body:?}"
    );
    assert_eq!(
        body["Reason"].as_str(),
        Some("SERVICE_QUOTA_EXCEEDED"),
        "case \"created past the quota\": expected Reason SERVICE_QUOTA_EXCEEDED, got {:?}",
        body["Reason"]
    );
    assert_eq!(
        body["Identifier"].as_str(),
        None,
        "case \"created past the quota\": a refused create must mint no rule, got {body:?}"
    );

    // A key named twice at creation is ONE tag with the last value given, the same
    // upsert TagResource applies — so a rule can never carry two tags for one key.
    let (_identifier, duplicate_arn) = rule_with_tags(
        &router,
        "the same key twice at creation",
        &pairs(&[("env", "dev"), ("team", "platform"), ("env", "prod")]),
    )
    .await;
    let (_status, listed) = list_tags(&router, &duplicate_arn).await;
    assert_eq!(
        tags_of(&listed, "Tags"),
        expected(&[("env", "prod"), ("team", "platform")]),
        "case \"the same key twice at creation\": expected one tag per key, got {:?}",
        listed["Tags"]
    );
}

#[tokio::test]
async fn the_tag_operations_answer_resource_not_found_for_an_arn_no_rule_carries() {
    let router = router();
    // One stored rule, so the store is not simply empty, plus one that is deleted.
    let (_identifier, stored_arn) = rule_with_tags(
        &router,
        "the stored rule",
        &pairs(&[("env", "dev")]),
    )
    .await;
    let (deleted_identifier, deleted_arn) = rule(&router, "about to be deleted").await;
    let (status, _) = delete_rule(&router, &deleted_identifier).await;
    assert_eq!(status, 204, "fixture DeleteRule returned {status}");

    // Every row is a well-formed `RuleArn` (the model's pattern) that no stored rule
    // carries: an unknown identifier, a deleted rule's ARN, and the right identifier in
    // the wrong Region or account.
    let identifier = stored_arn
        .rsplit('/')
        .next()
        .expect("the stored ARN ends in /{identifier}")
        .to_string();
    let rows: Vec<(&str, String)> = vec![
        (
            "an identifier no rule was minted with",
            format!("arn:aws:rbin:{REGION}:{ACCOUNT_ID}:rule/abcdefghijk"),
        ),
        ("a deleted rule's ARN", deleted_arn),
        (
            "the right identifier in another Region",
            format!("arn:aws:rbin:us-east-2:{ACCOUNT_ID}:rule/{identifier}"),
        ),
        (
            "the right identifier in another account",
            format!("arn:aws:rbin:{REGION}:111122223333:rule/{identifier}"),
        ),
    ];

    for (case, arn) in rows {
        let (status, body) = list_tags(&router, &arn).await;
        assert_eq!(
            status, 404,
            "case {case:?}: ListTagsForResource expected HTTP 404, got {status} / {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: ListTagsForResource expected ResourceNotFoundException, got {body:?}"
        );
        assert_eq!(
            body["Message"].as_str(),
            Some(
                format!(
                    "Recycle Bin Rule '{}' does not exist.",
                    arn.rsplit('/').next().unwrap_or(&arn)
                )
                .as_str()
            ),
            "case {case:?}: ListTagsForResource must name the rule identifier the ARN \
             addressed, got {:?}",
            body["Message"]
        );

        let (status, body) = tag_resource(&router, &arn, &pairs(&[("env", "dev")])).await;
        assert_eq!(
            status, 404,
            "case {case:?}: TagResource expected HTTP 404, got {status} / {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: TagResource expected ResourceNotFoundException, got {body:?}"
        );

        let (status, body) = untag_resource(&router, &arn, &keys(&["env"])).await;
        assert_eq!(
            status, 404,
            "case {case:?}: UntagResource expected HTTP 404, got {status} / {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: UntagResource expected ResourceNotFoundException, got {body:?}"
        );
    }

    // None of those refusals may have touched the rule that does exist.
    let (_status, listed) = list_tags(&router, &stored_arn).await;
    assert_eq!(
        tags_of(&listed, "Tags"),
        expected(&[("env", "dev")]),
        "the stored rule's tags must survive every refused call, got {:?}",
        listed["Tags"]
    );
}
