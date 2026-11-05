// SPDX-License-Identifier: CC0-1.0

//! The Aurora DSQL tagging surface — `ListTagsForResource` and `UntagResource` — end
//! to end over the wire.
//!
//! RULE under test for the reader: `ListTagsForResource` resolves `resourceArn` to a
//! resource this service holds and reports exactly the tag map persisted on it — the
//! empty map for a resource with no tags, the tagging surface's own `NotFoundException`
//! (404) for a well-formed Aurora DSQL ARN that names no stored resource, and
//! `ValidationException` (400) for an ARN that names no Aurora DSQL resource at all.
//!
//! RULE under test for the writer: `UntagResource`'s `tagKeys` names an END STATE, so
//! afterwards the resource carries none of the named keys and every other key it held;
//! the call therefore succeeds whether or not it removed anything and a repeat changes
//! nothing further. It addresses its resource by the same ARN read, so it answers the
//! same 404 and 400 for the same classes of ARN.
//!
//! The tables span the class each rule generalises over: the number of tags (none,
//! one, several, `TagMap`'s maximum), their size (`TagKey` / `TagValue` at their
//! published bounds), their characters (every class `TagKey`'s pattern allows), the
//! number of keys removed (one, several, all, a repeat, never-held, `TagKeyList`'s
//! maximum and one past it), and the forms an unresolvable ARN takes. The recorded
//! case — a cluster created with tags, then a missing key removed from it — is one row.
//!
//! The client is the `Dsql` harness in `harness.rs`: one test client per crate,
//! driving `MultiServiceRouter::dispatch` exactly as an SDK does, and one place
//! where the modelled tag bounds are spelled.

use serde_json::{json, Value};

use crate::harness::{
    absent_identifier, expected_arn, percent_encode_label, wait_for_the_clock, Answer, Dsql,
    TAG_KEY_MAX, TAG_MAP_MAX, TAG_VALUE_MAX,
};

/// `TagKeyList`'s published maximum. Source: `smithy_query(query="shape",
/// shape="TagKeyList")` — `@length(min: 0, max: 200)`.
///
/// The published minimum (0) is not reachable over the wire: `tagKeys` is `@required`
/// and bound to `@httpQuery`, and that binding is what tells `DELETE
/// /tags/{resourceArn}` apart from the other operations bound to the same path, so a
/// request carrying no `tagKeys` pair at all names no operation. The fewest keys a
/// request can express is one.
const TAG_KEYS_MAX: usize = 200;

/// Percent-encode a tag key for ONE `tagKeys` QUERY value.
///
/// `harness::percent_encode_label` encodes what a path label cannot carry (`:` and
/// `/`); a query value must also encode ` `, `+` and `=`, since `+` decodes to a
/// space on this wire and `=` separates the pair — and `TagKey`'s pattern
/// (`^[a-zA-Z0-9_.:/=+\-@ ]*$`) allows all three.
fn query_value(value: &str) -> String {
    percent_encode_label(value)
        .chars()
        .map(|c| match c {
            ' ' => "%20".to_string(),
            '+' => "%2B".to_string(),
            '=' => "%3D".to_string(),
            other => other.to_string(),
        })
        .collect()
}

/// `GET /tags/{resourceArn}` for `arn`.
async fn list_tags(dsql: &Dsql, arn: &str) -> Answer {
    dsql.list_tags_for_resource(arn).await
}

/// `DELETE /tags/{resourceArn}?tagKeys=…` — one `tagKeys` pair per key, which is how
/// the modelled `@httpQuery` list travels. Every caller names at least one key: a
/// request carrying no `tagKeys` pair carries no value for a `@required`
/// `@httpQuery` member, which is what tells this route from the other operations
/// bound to the same path, so it names no operation at all.
pub async fn untag(dsql: &Dsql, arn: &str, keys: &[String]) -> Answer {
    assert!(
        !keys.is_empty(),
        "UntagResource is addressed with at least one tagKeys pair"
    );
    let query = keys
        .iter()
        .map(|key| format!("tagKeys={}", query_value(key)))
        .collect::<Vec<_>>()
        .join("&");
    dsql.call(
        "DELETE",
        &format!("/tags/{}?{query}", percent_encode_label(arn)),
        None,
    )
    .await
}

/// One `CreateCluster` tag map and what `ListTagsForResource` must then report.
struct TagCase {
    name: String,
    region: &'static str,
    /// The `tags` member of the `CreateCluster` request, or `Value::Null` for a
    /// request that carries none.
    tags: Value,
    /// The map `ListTagsForResource` must answer with.
    expected: Value,
}

fn tag_cases() -> Vec<TagCase> {
    // Every character class `TagKey`'s pattern allows, in one key.
    let every_class_key = "aZ9_.:/=+-@ x";
    let long_key = "k".repeat(TAG_KEY_MAX);
    let long_value = "v".repeat(TAG_VALUE_MAX);
    // `TagMap` at its published maximum.
    let full_map: serde_json::Map<String, Value> = (0..TAG_MAP_MAX)
        .map(|n| (format!("key-{n}"), json!(format!("value-{n}"))))
        .collect();

    vec![
        TagCase {
            name: "a cluster created with no tags at all reports the empty map".to_string(),
            region: "us-east-1",
            tags: Value::Null,
            expected: json!({}),
        },
        TagCase {
            name: "an explicitly empty tag map reports the empty map".to_string(),
            region: "eu-west-1",
            tags: json!({}),
            expected: json!({}),
        },
        TagCase {
            name: "one tag".to_string(),
            region: "us-west-2",
            tags: json!({"env": "test"}),
            expected: json!({"env": "test"}),
        },
        TagCase {
            name: "several tags".to_string(),
            region: "ap-southeast-2",
            tags: json!({"env": "prod", "owner": "team-a", "cost-centre": "42"}),
            expected: json!({"env": "prod", "owner": "team-a", "cost-centre": "42"}),
        },
        TagCase {
            name: "a key using every character class TagKey's pattern allows".to_string(),
            region: "us-east-2",
            tags: json!({every_class_key: every_class_key}),
            expected: json!({every_class_key: every_class_key}),
        },
        TagCase {
            name: format!("a value at TagValue's {}-character minimum", 0),
            region: "eu-central-1",
            tags: json!({"empty": ""}),
            expected: json!({"empty": ""}),
        },
        TagCase {
            name: format!("a key at TagKey's {TAG_KEY_MAX}-character maximum"),
            region: "us-west-1",
            tags: json!({long_key.clone(): "at-the-bound"}),
            expected: json!({long_key: "at-the-bound"}),
        },
        TagCase {
            name: format!("a value at TagValue's {TAG_VALUE_MAX}-character maximum"),
            region: "ca-central-1",
            tags: json!({"long": long_value.clone()}),
            expected: json!({"long": long_value}),
        },
        TagCase {
            name: format!("{TAG_MAP_MAX} tags, TagMap's maximum"),
            region: "us-east-1",
            tags: Value::Object(full_map.clone()),
            expected: Value::Object(full_map),
        },
    ]
}

/// RULE: the tags `ListTagsForResource` reports are exactly the ones persisted on the
/// resource the ARN names — for every size, count and character class of tag map,
/// and in every region.
#[tokio::test]
async fn list_tags_reports_exactly_the_tags_persisted_on_the_resource() {
    for case in tag_cases() {
        let dsql = Dsql::in_region(case.region);
        let mut request = json!({"deletionProtectionEnabled": false});
        if !case.tags.is_null() {
            request["tags"] = case.tags.clone();
        }
        let created = dsql.create(request).await;
        assert_eq!(
            created.status, 200,
            "case {:?}: CreateCluster must answer 200, got {} {:?}",
            case.name, created.status, created.body
        );
        let identifier = created.str("identifier");
        let arn = created.str("arn");
        assert_eq!(
            arn,
            expected_arn(case.region, &identifier),
            "case {:?}: CreateCluster must answer the cluster's own ARN, got {arn:?}",
            case.name
        );

        let listed = list_tags(&dsql, &arn).await;
        assert_eq!(
            listed.status, 200,
            "case {:?}: ListTagsForResource must answer 200, got {} {:?}",
            case.name, listed.status, listed.body
        );
        assert_eq!(
            listed.get("tags"),
            case.expected,
            "case {:?}: ListTagsForResource must report the persisted tags, got {:?}",
            case.name,
            listed.get("tags")
        );

        // The same state read through the other reader of it: GetCluster omits the
        // member when the cluster carries no tags, and otherwise reports the map
        // ListTagsForResource just reported.
        let fetched = dsql.get(&identifier).await;
        let from_get = fetched.get("tags");
        let expected_from_get = if case.expected == json!({}) {
            Value::Null
        } else {
            case.expected.clone()
        };
        assert_eq!(
            from_get, expected_from_get,
            "case {:?}: GetCluster must agree with ListTagsForResource, got {from_get:?}",
            case.name
        );
    }
}

/// RULE: listing tags is a read — it changes nothing a later caller sees, including
/// the cluster's status and its tags.
#[tokio::test]
async fn listing_tags_changes_nothing_a_later_caller_sees() {
    let dsql = Dsql::new();
    let created = dsql
        .create(json!({"deletionProtectionEnabled": false, "tags": {"env": "test"}}))
        .await;
    assert_eq!(
        created.status, 200,
        "CreateCluster must answer 200, got {} {:?}",
        created.status, created.body
    );
    let identifier = created.str("identifier");
    let arn = created.str("arn");
    wait_for_the_clock().await;

    let before = dsql.get(&identifier).await;
    let first = list_tags(&dsql, &arn).await;
    let second = list_tags(&dsql, &arn).await;
    let after = dsql.get(&identifier).await;

    assert_eq!(
        first.get("tags"),
        second.get("tags"),
        "two reads must answer the same tags, got {:?} then {:?}",
        first.get("tags"),
        second.get("tags")
    );
    assert_eq!(
        after.body, before.body,
        "a read must leave the cluster exactly as it was, got {:?} after {:?}",
        after.body, before.body
    );
}

/// RULE: a well-formed Aurora DSQL ARN that names no resource this service holds
/// answers the ARN-addressed tagging surface's own 404 `NotFoundException` with its
/// fixed sentence.
#[tokio::test]
async fn an_arn_naming_no_stored_resource_is_not_found() {
    let dsql = Dsql::new();
    // A cluster that DOES exist, so a 404 is about the ARN and not about the store.
    let live = dsql.create_settled().await;
    let live_arn = expected_arn("us-east-1", &live);
    assert_eq!(
        list_tags(&dsql, &live_arn).await.status,
        200,
        "the live cluster's own ARN must resolve"
    );

    // A cluster whose deletion the clock has already completed: its ARN resolves to
    // nothing afterwards.
    let retired = dsql.create_settled().await;
    let retired_arn = expected_arn("us-east-1", &retired);
    let deleted = dsql.call("DELETE", &format!("/cluster/{retired}"), None).await;
    assert_eq!(
        deleted.status, 200,
        "DeleteCluster must answer 200, got {} {:?}",
        deleted.status, deleted.body
    );
    wait_for_the_clock().await;

    let cases: &[(&str, String)] = &[
        (
            "a cluster id this service never minted",
            expected_arn("us-east-1", &absent_identifier()),
        ),
        ("a retired cluster's ARN", retired_arn),
        (
            "another Aurora DSQL resource type this store holds none of",
            format!(
                "arn:aws:dsql:us-east-1:{}:stream/abcdefghijklmnopqrstuvwxyz",
                engine::EMULATOR_ACCOUNT_ID
            ),
        ),
        (
            "a cluster ARN written with a colon before the id",
            format!(
                "arn:aws:dsql:us-east-1:{}:cluster:{}",
                engine::EMULATOR_ACCOUNT_ID,
                absent_identifier()
            ),
        ),
    ];

    for (name, arn) in cases {
        let answer = list_tags(&dsql, arn).await;
        assert_eq!(
            answer.status, 404,
            "case {name:?}: ListTagsForResource must answer 404, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error.as_deref(),
            Some("NotFoundException"),
            "case {name:?}: the error code must be NotFoundException, got {:?}",
            answer.error
        );
        assert_eq!(
            answer.message(),
            "Resource was not found",
            "case {name:?}: the tagging surface's fixed sentence must state the refusal, \
             got {:?}",
            answer.message()
        );
    }

    // UntagResource addresses its resource through the same ARN read, so the same
    // class of ARN is the same 404 for it.
    for (name, arn) in cases {
        let answer = untag(&dsql, arn, &keys(&["env"])).await;
        assert_eq!(
            answer.status, 404,
            "case {name:?}: UntagResource must answer 404, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error.as_deref(),
            Some("NotFoundException"),
            "case {name:?}: the error code must be NotFoundException, got {:?}",
            answer.error
        );
    }
}

/// RULE: an ARN that names no Aurora DSQL resource at all is refused as invalid
/// input (400 `ValidationException`) rather than looked up.
#[tokio::test]
async fn an_arn_that_names_no_dsql_resource_is_refused_as_invalid() {
    let dsql = Dsql::new();
    let account = engine::EMULATOR_ACCOUNT_ID;
    // A cluster that DOES exist, so a refusal is about the ARN and not the store —
    // and so the untag half of the table has a tag it could have removed.
    let live = cluster_with_tags(&dsql, &json!({"env": "prod"})).await;
    let cases: &[(&str, String)] = &[
        (
            "another service's resource",
            format!("arn:aws:kinesis:us-east-1:{account}:stream/orders"),
        ),
        (
            "another service's resource carrying the live cluster's own id",
            format!("arn:aws:rds:us-east-1:{account}:cluster/{}", live.identifier),
        ),
        (
            "this service's namespace in another case",
            format!("arn:aws:DSQL:us-east-1:{account}:cluster/{}", live.identifier),
        ),
        (
            "an S3 ARN, with its empty region and account fields",
            "arn:aws:s3:::my-bucket/key".to_string(),
        ),
        (
            "a dsql ARN with no resource field",
            format!("arn:aws:dsql:us-east-1:{account}:"),
        ),
        (
            "a dsql ARN whose resource field has no type",
            format!("arn:aws:dsql:us-east-1:{account}:cluster"),
        ),
        (
            "a dsql ARN whose resource field has no id",
            format!("arn:aws:dsql:us-east-1:{account}:cluster/"),
        ),
        ("too few ARN fields", "arn:aws:dsql:us-east-1".to_string()),
        // `Arn`'s own `@pattern("^arn:.+$")`: refused at the boundary, before the
        // handler runs, with the same status and code.
        ("not an ARN at all", "cluster/abc".to_string()),
    ];

    for (name, arn) in cases {
        let answer = list_tags(&dsql, arn).await;
        assert_eq!(
            answer.status, 400,
            "case {name:?}: ListTagsForResource must answer 400, got {} {:?}",
            answer.status, answer.body
        );
        assert!(
            answer
                .error
                .as_deref()
                .is_some_and(|code| code.contains("ValidationException")),
            "case {name:?}: the error code must be ValidationException, got {:?}",
            answer.error
        );

        // Same ARN read, same refusal for the writer — and a refused call removes
        // nothing: the live cluster still carries the key the request named.
        let removal = untag(&dsql, arn, &keys(&["env"])).await;
        assert_eq!(
            removal.status, 400,
            "case {name:?}: UntagResource must answer 400, got {} {:?}",
            removal.status, removal.body
        );
        assert!(
            removal
                .error
                .as_deref()
                .is_some_and(|code| code.contains("ValidationException")),
            "case {name:?}: the error code must be ValidationException, got {:?}",
            removal.error
        );
        let still = list_tags(&dsql, &live.arn).await;
        assert_eq!(
            still.get("tags"),
            json!({"env": "prod"}),
            "case {name:?}: a refused call must remove no tag, got {:?}",
            still.get("tags")
        );
    }
}

/// A cluster created for a removal case: the identifier `GetCluster` reads it by and
/// the ARN the tagging operations address it by.
struct TaggedCluster {
    identifier: String,
    arn: String,
}

/// Create a cluster carrying `tags`, settled past the clock's `CREATING -> ACTIVE`
/// transition so nothing else is pending while the tags are rewritten.
///
/// Not `Dsql::create_settled`: that one posts its own untagged request body and
/// answers the identifier alone, while every case here needs a tag map on the
/// request and the ARN the tagging operations address the cluster by.
async fn cluster_with_tags(dsql: &Dsql, tags: &Value) -> TaggedCluster {
    let created = dsql
        .create(json!({"deletionProtectionEnabled": false, "tags": tags}))
        .await;
    assert_eq!(
        created.status, 200,
        "CreateCluster must answer 200, got {} {:?}",
        created.status, created.body
    );
    wait_for_the_clock().await;
    TaggedCluster {
        identifier: created.str("identifier"),
        arn: created.str("arn"),
    }
}

/// `tagKeys` as the wire carries them.
fn keys(values: &[&str]) -> Vec<String> {
    values.iter().map(|key| (*key).to_string()).collect()
}

/// One `UntagResource` call against a tagged cluster, and the tags the cluster must
/// carry afterwards.
struct UntagCase {
    name: String,
    /// The tags the cluster is created with.
    held: Value,
    /// The `tagKeys` the call names.
    removed: Vec<String>,
    /// What the resource must report afterwards.
    remaining: Value,
}

fn untag_cases() -> Vec<UntagCase> {
    // Every key below is drawn from `TagKey`'s own character class,
    // `^[a-zA-Z0-9_.:/=+\-@ ]*$` (source: `smithy_query(query="shape",
    // shape="TagKey")`), so no row is refused for a reason other than the rule.
    let three = json!({"env": "prod", "owner": "team-a", "cost-centre": "42"});
    let punctuated = json!({
        "a key with spaces": "v1",
        "ns:sub/part": "v2",
        "plus+equals=": "v3",
        "mail@example.com": "v4",
        "_under.scored-": "v5",
    });
    let long_key = "k".repeat(TAG_KEY_MAX);
    let full_map: serde_json::Map<String, Value> = (0..TAG_KEYS_MAX)
        .map(|n| (format!("key{n:03}"), json!(format!("value{n:03}"))))
        .collect();
    let every_other: serde_json::Map<String, Value> = full_map
        .iter()
        .enumerate()
        .filter(|(n, _)| n % 2 == 1)
        .map(|(_, (key, value))| (key.clone(), value.clone()))
        .collect();

    vec![
        UntagCase {
            name: "the recorded class: a key the resource never carried".to_string(),
            held: three.clone(),
            removed: keys(&["never-set"]),
            remaining: three.clone(),
        },
        UntagCase {
            name: "a never-carried key, on a resource with no tags at all".to_string(),
            held: json!({}),
            removed: keys(&["never-set"]),
            remaining: json!({}),
        },
        UntagCase {
            name: "one key, the fewest a request can express".to_string(),
            held: three.clone(),
            removed: keys(&["env"]),
            remaining: json!({"owner": "team-a", "cost-centre": "42"}),
        },
        UntagCase {
            name: "several held keys at once".to_string(),
            held: three.clone(),
            removed: keys(&["owner", "cost-centre"]),
            remaining: json!({"env": "prod"}),
        },
        UntagCase {
            name: "every held key".to_string(),
            held: three.clone(),
            removed: keys(&["env", "owner", "cost-centre"]),
            remaining: json!({}),
        },
        UntagCase {
            name: "a mixture of held and never-held keys".to_string(),
            held: three.clone(),
            removed: keys(&["env", "never-set", "cost-centre", "also-never-set"]),
            remaining: json!({"owner": "team-a"}),
        },
        UntagCase {
            name: "the same key named twice in one call".to_string(),
            held: three.clone(),
            removed: keys(&["env", "env"]),
            remaining: json!({"owner": "team-a", "cost-centre": "42"}),
        },
        UntagCase {
            name: "keys are case-sensitive, so only the exact spelling goes".to_string(),
            held: json!({"Env": "prod", "env": "test", "ENV": "dev"}),
            removed: keys(&["env"]),
            remaining: json!({"Env": "prod", "ENV": "dev"}),
        },
        UntagCase {
            name: "keys carrying every punctuation TagKey allows".to_string(),
            held: punctuated,
            removed: keys(&["a key with spaces", "ns:sub/part", "plus+equals="]),
            remaining: json!({"mail@example.com": "v4", "_under.scored-": "v5"}),
        },
        UntagCase {
            name: format!("a key at TagKey's {TAG_KEY_MAX}-character maximum"),
            held: json!({"env": "prod", long_key.clone(): "at-the-bound"}),
            removed: vec![long_key],
            remaining: json!({"env": "prod"}),
        },
        UntagCase {
            name: format!("{TAG_KEYS_MAX} keys, TagKeyList's maximum, removing every tag held"),
            held: Value::Object(full_map.clone()),
            removed: full_map.keys().cloned().collect(),
            remaining: json!({}),
        },
        UntagCase {
            name: "half of a large tag set".to_string(),
            held: Value::Object(full_map.clone()),
            removed: full_map
                .keys()
                .enumerate()
                .filter(|(n, _)| n % 2 == 0)
                .map(|(_, key)| key.clone())
                .collect(),
            remaining: Value::Object(every_other),
        },
    ]
}

/// RULE: `UntagResource` leaves the resource carrying none of the keys it named and
/// every other key it held, and because that is an END STATE the call succeeds
/// whether or not it removed anything and a repeat changes nothing further.
#[tokio::test]
async fn untagging_removes_exactly_the_named_keys_and_is_idempotent() {
    for case in untag_cases() {
        let dsql = Dsql::new();
        let cluster = cluster_with_tags(&dsql, &case.held).await;

        for attempt in 1..=2 {
            let answer = untag(&dsql, &cluster.arn, &case.removed).await;
            assert_eq!(
                answer.status, 200,
                "case {:?} (attempt {attempt}): UntagResource must answer 200, got {} {:?}",
                case.name, answer.status, answer.body
            );
            assert!(
                answer.error.is_none(),
                "case {:?} (attempt {attempt}): expected no error, got {:?}",
                case.name,
                answer.error
            );

            let listed = list_tags(&dsql, &cluster.arn).await;
            assert_eq!(
                listed.get("tags"),
                case.remaining,
                "case {:?} (attempt {attempt}): expected tags {:?}, got {:?}",
                case.name,
                case.remaining,
                listed.get("tags")
            );

            // The other reader of the same state must agree: GetCluster omits the
            // member when nothing is left.
            let fetched = dsql.get(&cluster.identifier).await;
            let expected_from_get = if case.remaining == json!({}) {
                Value::Null
            } else {
                case.remaining.clone()
            };
            assert_eq!(
                fetched.get("tags"),
                expected_from_get,
                "case {:?} (attempt {attempt}): GetCluster must agree, got {:?}",
                case.name,
                fetched.get("tags")
            );
        }
    }
}

/// RULE: untagging touches only the resource its ARN names — no other resource's
/// tags, and nothing else about the resource it does name.
#[tokio::test]
async fn untagging_touches_only_the_resource_the_arn_names() {
    let dsql = Dsql::in_region("eu-west-1");
    let first = cluster_with_tags(&dsql, &json!({"env": "prod", "owner": "team-a"})).await;
    let second = cluster_with_tags(&dsql, &json!({"env": "test", "owner": "team-b"})).await;

    let before = dsql.get(&first.identifier).await;
    let answer = untag(&dsql, &first.arn, &keys(&["env"])).await;
    assert_eq!(
        answer.status, 200,
        "UntagResource must answer 200, got {} {:?}",
        answer.status, answer.body
    );

    let after = dsql.get(&first.identifier).await;
    assert_eq!(
        after.get("tags"),
        json!({"owner": "team-a"}),
        "case \"the named cluster\": only the named key may go, got {:?}",
        after.get("tags")
    );
    for member in ["identifier", "arn", "status", "creationTime", "endpoint"] {
        assert_eq!(
            after.get(member),
            before.get(member),
            "case {member:?}: untagging must not change it, got {:?} (was {:?})",
            after.get(member),
            before.get(member)
        );
    }

    let untouched = list_tags(&dsql, &second.arn).await;
    assert_eq!(
        untouched.get("tags"),
        json!({"env": "test", "owner": "team-b"}),
        "case \"another cluster\": its tags must be untouched, got {:?}",
        untouched.get("tags")
    );
}

/// RULE: `tagKeys`' published maximum is the boundary's, so a list up to it is served
/// and one past it is refused.
#[tokio::test]
async fn the_published_tag_keys_bound_is_enforced() {
    let dsql = Dsql::new();
    let cluster = cluster_with_tags(&dsql, &json!({"env": "prod"})).await;

    // (case, how many keys the call names, the status it must answer)
    let rows: &[(&str, usize, u16)] = &[
        ("the fewest a request can express", 1, 200),
        ("a handful", 5, 200),
        ("the published maximum", TAG_KEYS_MAX, 200),
        ("one past the published maximum", TAG_KEYS_MAX + 1, 400),
    ];

    for (case, count, status) in rows {
        // Keys the cluster does not hold, so only the bound decides the answer.
        let named: Vec<String> = (0..*count).map(|n| format!("absent{n:03}")).collect();
        let answer = untag(&dsql, &cluster.arn, &named).await;
        assert_eq!(
            answer.status, *status,
            "case {case:?} ({count} keys): expected HTTP {status}, got {} {:?}",
            answer.status, answer.body
        );
    }

    let listed = list_tags(&dsql, &cluster.arn).await;
    assert_eq!(
        listed.get("tags"),
        json!({"env": "prod"}),
        "no key the cluster holds was named, so its tags must be unchanged, got {:?}",
        listed.get("tags")
    );
}

/// A `targetDefinition` naming a Kinesis destination, as `CreateStream` requires
/// (`KinesisTargetDefinition`: a `streamArn` and a `roleArn`).
fn kinesis_target(region: &str) -> Value {
    let account = engine::EMULATOR_ACCOUNT_ID;
    json!({"kinesis": {
        "streamArn": format!("arn:aws:kinesis:{region}:{account}:stream/changes"),
        "roleArn": format!("arn:aws:iam::{account}:role/dsql-cdc"),
    }})
}

/// RULE: the tagging ARN read resolves EVERY resource of this service that carries
/// tags, each to its OWN tag map — so a stream of a cluster, whose ARN this service
/// mints and hands back, is listed and untagged exactly as a cluster is, and
/// untagging one does not touch the other.
#[tokio::test]
async fn a_stream_arn_addresses_the_streams_own_tags() {
    let region = "us-east-1";
    let dsql = Dsql::in_region(region);
    let cluster = cluster_with_tags(&dsql, &json!({"env": "prod", "owner": "team-a"})).await;

    let created = dsql
        .create_stream(
            &cluster.identifier,
            json!({
                "format": "JSON",
                "ordering": "UNORDERED",
                "targetDefinition": kinesis_target(region),
                "tags": {"env": "test", "scope": "cdc"},
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "CreateStream must answer 200, got {} {:?}",
        created.status, created.body
    );
    let stream_arn = created.str("arn");
    let stream_identifier = created.str("streamIdentifier");
    wait_for_the_clock().await;

    let listed = list_tags(&dsql, &stream_arn).await;
    assert_eq!(
        listed.get("tags"),
        json!({"env": "test", "scope": "cdc"}),
        "case \"a stream ARN\": the stream's own tags must be reported, got {:?}",
        listed.get("tags")
    );

    let removed = untag(&dsql, &stream_arn, &keys(&["env", "never-set"])).await;
    assert_eq!(
        removed.status, 200,
        "case \"a stream ARN\": UntagResource must answer 200, got {} {:?}",
        removed.status, removed.body
    );
    let after = list_tags(&dsql, &stream_arn).await;
    assert_eq!(
        after.get("tags"),
        json!({"scope": "cdc"}),
        "case \"a stream ARN\": only the named key may go, got {:?}",
        after.get("tags")
    );
    let cluster_tags = list_tags(&dsql, &cluster.arn).await;
    assert_eq!(
        cluster_tags.get("tags"),
        json!({"env": "prod", "owner": "team-a"}),
        "case \"the owning cluster\": its tags must be untouched, got {:?}",
        cluster_tags.get("tags")
    );

    // A stream of this cluster that was never created names nothing, so both
    // operations answer the tagging surface's 404 for it.
    let absent = format!(
        "arn:aws:dsql:{region}:{}:cluster/{}/stream/{}",
        engine::EMULATOR_ACCOUNT_ID,
        cluster.identifier,
        absent_identifier()
    );
    assert_ne!(
        absent, stream_arn,
        "the absent stream's ARN must differ from the created one"
    );
    for (case, answer) in [
        ("ListTagsForResource", list_tags(&dsql, &absent).await),
        ("UntagResource", untag(&dsql, &absent, &keys(&["env"])).await),
    ] {
        assert_eq!(
            answer.status, 404,
            "case {case:?}: an absent stream must answer 404, got {} {:?}",
            answer.status, answer.body
        );
    }

    // And the created stream is still reachable by its identifier, so nothing above
    // disturbed it.
    let fetched = dsql.get_stream(&cluster.identifier, &stream_identifier).await;
    assert_eq!(
        fetched.status, 200,
        "GetStream must still answer 200, got {} {:?}",
        fetched.status, fetched.body
    );
}
