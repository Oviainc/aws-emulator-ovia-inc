// SPDX-License-Identifier: CC0-1.0

//! `UntagResource` removes from a resource exactly the tag keys the request names.
//!
//! The rule under test, stated once: `UntagResource` deletes from the tag map persisted
//! for the resource its `ResourceArn` names exactly the keys `TagKeys` names and nothing
//! else — a key the resource does not carry removes nothing, so the call is idempotent —
//! then answers the modelled 204 with no body; the reduced map is what every later read
//! of that resource (`ListTagsForResource` and the resource's own describe) reports, the
//! resource itself is otherwise untouched, and a `ResourceArn` naming no resource this
//! service holds in the Region the request is served in is answered `NotFoundException`
//! (HTTP 404) with nothing removed anywhere.
//!
//! Each test is a TABLE. The rows span the three resource types this service's ARNs name
//! (`smithy_query(query="digest")` — PackagingGroup, PackagingConfiguration, Asset) and
//! the ways a removal can relate to the stored map: one key of several, several of
//! several, every key, the only key, a key that is not there at all, a mixture of present
//! and absent keys, keys differing only in case, a key repeated in one request, keys
//! carrying spaces and punctuation, non-ASCII keys, and a resource carrying no tags.
//! `TagKeys` is an `@httpQuery` list bound to `tagKeys`
//! (`smithy_query(query="operation",operation="UntagResource")`), so each key travels as
//! one `tagKeys=` parameter with its reserved characters percent-encoded.
//!
//! Every case drives real HTTP through the router the production binary builds. The
//! harness (`router`, `call`, `KINDS`, `tags_route`, `tag_map`, the creates) is the one
//! `tests/all/resource_tag_listing.rs` already defines for the tag slice.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_mediapackagevod_mediapackagevod::state as vod;

use crate::resource_tag_listing::{
    call, create_parent_group, create_with_tags, expected_map, router, tag_map, tags_route, KINDS,
    OTHER_REGION, PARENT_GROUP, REGION, SOURCE_ARN,
};

/// The modelled refusal of a resource this service does not hold
/// (`smithy_query(query="shape",shape="NotFoundException")` — `httpError 404`).
const NOT_FOUND: (u16, &str) = (404, "NotFoundException");

/// The modelled success of `UntagResource`: `@http` code 204, so no body at all
/// (`smithy_query(query="operation",operation="UntagResource")`).
const NO_CONTENT: u16 = 204;

/// One query-parameter value, percent-encoded as an SDK sends it: everything outside
/// RFC 3986's unreserved set is escaped, so a key carrying a space, a `/` or non-ASCII
/// text travels intact.
///
/// Written out rather than pulled from `percent-encoding`, which this crate does not
/// depend on: this is the whole of the encoding a `tagKeys=` parameter needs.
fn urlencode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// The `DELETE /tags/{ResourceArn}?tagKeys=...` route that removes `keys` from `arn`.
fn untag_route(arn: &str, keys: &[&str]) -> String {
    let query = keys
        .iter()
        .map(|key| format!("tagKeys={}", urlencode(key)))
        .collect::<Vec<_>>()
        .join("&");
    if query.is_empty() {
        tags_route(arn)
    } else {
        format!("{}?{query}", tags_route(arn))
    }
}

/// Remove `keys` from `arn` in `region` and report what the call answered.
async fn untag(
    router: &MultiServiceRouter,
    region: &str,
    arn: &str,
    keys: &[&str],
) -> crate::resource_tag_listing::Answer {
    call(router, region, "DELETE", &untag_route(arn, keys), None).await
}

/// The tag map `ListTagsForResource` reports for `arn` in `region`, asserting the read
/// itself succeeded.
async fn listed_tags(
    router: &MultiServiceRouter,
    region: &str,
    arn: &str,
    case: &str,
) -> BTreeMap<String, String> {
    let listed = call(router, region, "GET", &tags_route(arn), None).await;
    assert_eq!(
        listed.status, 200,
        "case {case:?}: ListTagsForResource expected 200, got {} body {}",
        listed.status, listed.body
    );
    tag_map(&listed.body)
}

/// Assert one successful `UntagResource`: the modelled 204, and no body with it.
fn assert_no_content(answer: &crate::resource_tag_listing::Answer, case: &str) {
    assert_eq!(
        answer.status, NO_CONTENT,
        "case {case:?}: UntagResource expected {NO_CONTENT}, got {} body {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.body,
        Value::Null,
        "case {case:?}: a 204 carries no body, got {}",
        answer.body
    );
}

/// One removal row: the tags the resource is created with, the keys the untag names, and
/// the map that must remain.
struct Row {
    case: &'static str,
    created: Option<Value>,
    remove: &'static [&'static str],
    remaining: Value,
}

/// The removals the table spans. `Tags` is a `__mapOf__string` and `TagKeys` a
/// `__listOf__string`, neither carrying a declared constraint
/// (`smithy_query(query="shape",shape="UntagResourceRequest")`), so every key below is
/// one the service accepts.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one key of several",
            created: Some(json!({ "environment": "production", "team": "video", "tier": "gold" })),
            remove: &["team"],
            remaining: json!({ "environment": "production", "tier": "gold" }),
        },
        Row {
            case: "several keys of several",
            created: Some(json!({ "a": "1", "b": "2", "c": "3", "d": "4" })),
            remove: &["a", "c"],
            remaining: json!({ "b": "2", "d": "4" }),
        },
        Row {
            case: "every key the resource carries",
            created: Some(json!({ "environment": "staging", "team": "video" })),
            remove: &["environment", "team"],
            remaining: json!({}),
        },
        Row {
            case: "the only key the resource carries",
            created: Some(json!({ "environment": "production" })),
            remove: &["environment"],
            remaining: json!({}),
        },
        Row {
            case: "a key the resource does not carry",
            created: Some(json!({ "environment": "production" })),
            remove: &["never-applied"],
            remaining: json!({ "environment": "production" }),
        },
        Row {
            case: "a key it carries beside one it does not",
            created: Some(json!({ "environment": "production", "team": "video" })),
            remove: &["environment", "never-applied"],
            remaining: json!({ "team": "video" }),
        },
        Row {
            case: "keys differing only in case",
            created: Some(json!({ "environment": "production", "Environment": "staging" })),
            remove: &["environment"],
            remaining: json!({ "Environment": "staging" }),
        },
        Row {
            case: "the same key named twice in one request",
            created: Some(json!({ "environment": "production", "team": "video" })),
            remove: &["environment", "environment"],
            remaining: json!({ "team": "video" }),
        },
        Row {
            case: "a key carrying spaces and punctuation",
            created: Some(json!({ "Cost Center": "video / vod", "note": "renewed, 2026-01-01" })),
            remove: &["Cost Center"],
            remaining: json!({ "note": "renewed, 2026-01-01" }),
        },
        Row {
            case: "a non-ASCII key",
            created: Some(json!({ "região": "são-paulo", "プロジェクト": "配信" })),
            remove: &["região"],
            remaining: json!({ "プロジェクト": "配信" }),
        },
        Row {
            case: "a resource carrying no tags at all",
            created: None,
            remove: &["environment"],
            remaining: json!({}),
        },
    ]
}

/// Untagging removes exactly the named keys and nothing else, for every resource type
/// and every shape of removal — and the reduced map is what `ListTagsForResource` and the
/// resource's own describe both report afterwards.
#[tokio::test]
async fn untagging_removes_exactly_the_named_keys() {
    let router = router();
    create_parent_group(&router, REGION).await;

    for kind in KINDS {
        for (index, row) in rows().into_iter().enumerate() {
            let case = format!("{}/{}", kind.name, row.case);
            let id = format!("{}-remove-{index}", kind.arn_kind);
            let arn =
                create_with_tags(&router, REGION, kind, &id, row.created.as_ref(), &case).await;

            let answer = untag(&router, REGION, &arn, row.remove).await;
            assert_no_content(&answer, &case);

            let expected = expected_map(Some(&row.remaining));
            assert_eq!(
                listed_tags(&router, REGION, &arn, &case).await,
                expected,
                "case {case:?}: after removing {:?} expected {expected:?}",
                row.remove
            );

            // The one stored copy: the resource's own read reports the reduced map too,
            // and the resource is still there to be read.
            let described = call(&router, REGION, "GET", &kind.describe_route(&id), None).await;
            assert_eq!(
                described.status, 200,
                "case {case:?}: Describe{} expected 200, got {} body {}",
                kind.name, described.status, described.body
            );
            assert_eq!(
                tag_map(&described.body),
                expected,
                "case {case:?}: Describe{} must report {expected:?}, got {}",
                kind.name,
                described.body
            );
        }
    }
}

/// Untagging is idempotent: the same request repeated answers 204 again and leaves the
/// same tag map, for every resource type and whether the keys were there to begin with.
#[tokio::test]
async fn repeating_an_untag_changes_nothing_further() {
    let router = router();
    create_parent_group(&router, REGION).await;
    let created = json!({ "environment": "production", "team": "video", "tier": "gold" });

    // Rows: keys the resource carries, keys it does not, and a mixture.
    let removals: [(&str, &[&str], Value); 3] = [
        (
            "keys the resource carries",
            &["environment", "team"],
            json!({ "tier": "gold" }),
        ),
        (
            "keys the resource does not carry",
            &["absent-one", "absent-two"],
            json!({ "environment": "production", "team": "video", "tier": "gold" }),
        ),
        (
            "a present key beside an absent one",
            &["tier", "absent-one"],
            json!({ "environment": "production", "team": "video" }),
        ),
    ];

    for kind in KINDS {
        for (index, (row, remove, remaining)) in removals.iter().enumerate() {
            let case = format!("{}/{row}", kind.name);
            let id = format!("{}-idempotent-{index}", kind.arn_kind);
            let arn = create_with_tags(&router, REGION, kind, &id, Some(&created), &case).await;
            let expected = expected_map(Some(remaining));

            for attempt in 1..=3 {
                let answer = untag(&router, REGION, &arn, remove).await;
                assert_no_content(&answer, &format!("{case} attempt {attempt}"));
                assert_eq!(
                    listed_tags(&router, REGION, &arn, &case).await,
                    expected,
                    "case {case:?} attempt {attempt}: expected {expected:?} after removing {remove:?}"
                );
            }

            let described = call(&router, REGION, "GET", &kind.describe_route(&id), None).await;
            assert_eq!(
                (described.status, tag_map(&described.body)),
                (200, expected.clone()),
                "case {case:?}: the resource must survive the repeats, got {} body {}",
                described.status,
                described.body
            );
        }
    }
}

/// A `ResourceArn` naming no resource this service holds in the Region the request is
/// served in is answered `NotFoundException` (404) — never a 204, which would claim tags
/// were removed from a resource this service does not hold — and nothing is removed from
/// the resource it does hold.
#[tokio::test]
async fn untagging_an_arn_naming_no_resource_of_this_region_is_not_found() {
    let router = router();
    create_parent_group(&router, REGION).await;
    let account = engine::emulator_account_id();
    let held_tags = json!({ "environment": "production", "team": "video" });
    let held = create_with_tags(
        &router,
        REGION,
        KINDS[0],
        "vod-held-group",
        Some(&held_tags),
        "the resource the misses are compared against",
    )
    .await;

    // A resource of the SAME id exists in another Region, so a cross-Region untag that
    // answered 204 would have removed that other Region's tags.
    create_parent_group(&router, OTHER_REGION).await;

    let mut rows: Vec<(&str, String)> = vec![
        (
            "an id this service never created",
            vod::resource_arn(vod::ARN_KIND_ASSETS, REGION, "never-created"),
        ),
        (
            "another Region's resource of the same id",
            vod::resource_arn(vod::ARN_KIND_PACKAGING_GROUPS, OTHER_REGION, PARENT_GROUP),
        ),
        (
            "another account's ARN of a held resource",
            held.replace(account, "999999999999"),
        ),
        (
            "another partition's ARN of a held resource",
            held.replacen("arn:aws:", "arn:aws-cn:", 1),
        ),
        (
            "a resource type this service has none of",
            format!("arn:aws:mediapackage-vod:{REGION}:{account}:channels/anything"),
        ),
        ("an ARN of another service", SOURCE_ARN.to_string()),
        ("a value that is not an ARN at all", "not-an-arn".to_string()),
        (
            "an ARN carrying no id",
            format!("arn:aws:mediapackage-vod:{REGION}:{account}:assets/"),
        ),
    ];

    // A deleted resource is a miss too: its tags went with it.
    let doomed = create_with_tags(
        &router,
        REGION,
        KINDS[0],
        "vod-doomed-group",
        Some(&json!({ "environment": "scratch" })),
        "a resource that is then deleted",
    )
    .await;
    let deleted = call(
        &router,
        REGION,
        "DELETE",
        "/packaging_groups/vod-doomed-group",
        None,
    )
    .await;
    assert_eq!(
        deleted.status, 204,
        "DeletePackagingGroup: expected 204, got {} body {}",
        deleted.status, deleted.body
    );
    rows.push(("a resource that was deleted", doomed));

    for (case, arn) in rows {
        let answer = untag(&router, REGION, &arn, &["environment"]).await;
        assert_eq!(
            (answer.status, answer.errortype.as_str()),
            NOT_FOUND,
            "case {case:?} ({arn:?}): expected {NOT_FOUND:?}, got {} / {:?} body {}",
            answer.status,
            answer.errortype,
            answer.body
        );
        assert!(
            answer
                .body
                .get("message")
                .and_then(Value::as_str)
                .is_some_and(|message| message.contains("not found")),
            "case {case:?}: the refusal must say the resource was not found, got {}",
            answer.body
        );
    }

    // Every refusal left the resource this service DOES hold with all of its tags.
    let expected = expected_map(Some(&held_tags));
    assert_eq!(
        listed_tags(&router, REGION, &held, "the held resource after every miss").await,
        expected,
        "a refused untag must remove nothing: expected {expected:?}"
    );
}

/// An untag resolves only the resources of the Region it is served in: removing a key
/// from the resource of one Region leaves the same-named resource of another Region
/// carrying that key.
#[tokio::test]
async fn untagging_in_one_region_leaves_another_regions_tags_alone() {
    let router = router();
    let tags = json!({ "environment": "production", "team": "video" });

    for region in [REGION, OTHER_REGION] {
        create_with_tags(
            &router,
            region,
            KINDS[0],
            PARENT_GROUP,
            Some(&tags),
            &format!("{region}/the group both Regions hold"),
        )
        .await;
    }

    let arn_in = |region: &str| vod::resource_arn(vod::ARN_KIND_PACKAGING_GROUPS, region, PARENT_GROUP);
    let answer = untag(&router, REGION, &arn_in(REGION), &["environment"]).await;
    assert_no_content(&answer, "untag in the serving Region only");

    assert_eq!(
        listed_tags(&router, REGION, &arn_in(REGION), REGION).await,
        expected_map(Some(&json!({ "team": "video" }))),
        "the serving Region's group must have lost exactly the removed key"
    );
    assert_eq!(
        listed_tags(&router, OTHER_REGION, &arn_in(OTHER_REGION), OTHER_REGION).await,
        expected_map(Some(&tags)),
        "the other Region's group of the same id must keep every tag"
    );
}
