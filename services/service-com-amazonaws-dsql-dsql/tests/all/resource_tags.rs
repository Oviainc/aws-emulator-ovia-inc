// SPDX-License-Identifier: CC0-1.0

//! `ListTagsForResource`, end to end over the wire.
//!
//! RULE under test: `ListTagsForResource` resolves `resourceArn` to a resource this
//! service holds and reports exactly the tag map persisted on it — the empty map for
//! a resource with no tags, `ResourceNotFoundException` (404) for a well-formed
//! Aurora DSQL ARN that names no stored resource, and `ValidationException` (400) for
//! an ARN that names no Aurora DSQL resource at all.
//!
//! The tables span the class the rule generalises over: the number of tags (none,
//! one, several, `TagMap`'s maximum), their size (`TagKey` / `TagValue` at their
//! published bounds), their characters (every class `TagKey`'s pattern allows), and
//! the forms an unresolvable ARN takes. The recorded case — a cluster created with
//! tags, then listed — is one row.
//!
//! The client is the `Dsql` harness in `harness.rs`: one test client per crate,
//! driving `MultiServiceRouter::dispatch` exactly as an SDK does.

use serde_json::{json, Value};

use crate::harness::{absent_identifier, expected_arn, wait_for_the_clock, Answer, Dsql};

/// `TagKey` and `TagValue` bounds. Source: `smithy_query(query="shape",
/// shape="TagKey,TagValue")` — key `@length(1..128)`, value `@length(0..256)`, both
/// `@pattern("^[a-zA-Z0-9_.:/=+\\-@ ]*$")`.
const TAG_KEY_MAX: usize = 128;
const TAG_VALUE_MAX: usize = 256;
/// `TagMap`'s own `@length(0..200)`, same source.
const TAG_MAP_MAX: usize = 200;

/// Percent-encode a value for ONE `@httpLabel` path segment, the way an SDK encodes
/// a non-greedy label: `:` and `/` inside an ARN must not be read as segment
/// separators, and the other characters `TagKey`'s pattern allows are encoded too.
fn label(value: &str) -> String {
    value
        .chars()
        .map(|c| match c {
            ':' => "%3A".to_string(),
            '/' => "%2F".to_string(),
            ' ' => "%20".to_string(),
            '+' => "%2B".to_string(),
            '=' => "%3D".to_string(),
            '@' => "%40".to_string(),
            other => other.to_string(),
        })
        .collect()
}

/// `GET /tags/{resourceArn}` for `arn`.
async fn list_tags(dsql: &Dsql, arn: &str) -> Answer {
    dsql.call("GET", &format!("/tags/{}", label(arn)), None).await
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
/// answers the modelled `ResourceNotFoundException` (404), naming the resource it
/// could not find.
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

    let cases: &[(&str, String, &str, String)] = &[
        (
            "a cluster id this service never minted",
            expected_arn("us-east-1", &absent_identifier()),
            "cluster",
            absent_identifier(),
        ),
        (
            "a retired cluster's ARN",
            retired_arn,
            "cluster",
            retired.clone(),
        ),
        (
            "another Aurora DSQL resource type this store holds none of",
            format!(
                "arn:aws:dsql:us-east-1:{}:stream/abcdefghijklmnopqrstuvwxyz",
                engine::EMULATOR_ACCOUNT_ID
            ),
            "stream",
            "abcdefghijklmnopqrstuvwxyz".to_string(),
        ),
        (
            "a cluster ARN written with a colon before the id",
            format!(
                "arn:aws:dsql:us-east-1:{}:cluster:{}",
                engine::EMULATOR_ACCOUNT_ID,
                absent_identifier()
            ),
            "cluster",
            absent_identifier(),
        ),
    ];

    for (name, arn, resource_type, resource_id) in cases {
        let answer = list_tags(&dsql, arn).await;
        assert_eq!(
            answer.status, 404,
            "case {name:?}: ListTagsForResource must answer 404, got {} {:?}",
            answer.status, answer.body
        );
        assert!(
            answer
                .error
                .as_deref()
                .is_some_and(|code| code.contains("ResourceNotFoundException")),
            "case {name:?}: the error code must be ResourceNotFoundException, got {:?}",
            answer.error
        );
        assert_eq!(
            answer.str("resourceType"),
            *resource_type,
            "case {name:?}: the error must name the resource type, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.str("resourceId"),
            *resource_id,
            "case {name:?}: the error must name the resource id, got {:?}",
            answer.body
        );
    }
}

/// RULE: an ARN that names no Aurora DSQL resource at all is refused as invalid
/// input (400 `ValidationException`) rather than looked up.
#[tokio::test]
async fn an_arn_that_names_no_dsql_resource_is_refused_as_invalid() {
    let dsql = Dsql::new();
    let account = engine::EMULATOR_ACCOUNT_ID;
    let cases: &[(&str, String)] = &[
        (
            "another service's resource",
            format!("arn:aws:kinesis:us-east-1:{account}:stream/orders"),
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
        (
            "too few ARN fields",
            "arn:aws:dsql:us-east-1".to_string(),
        ),
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
    }
}
