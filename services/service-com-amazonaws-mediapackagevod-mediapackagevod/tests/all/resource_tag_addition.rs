// SPDX-License-Identifier: CC0-1.0

//! `TagResource` adds to a resource exactly the tags the request names.
//!
//! The rule under test, stated once: `TagResource` MERGES the `Tags` map into the tag map
//! persisted for the resource its `ResourceArn` names — a key the resource does not carry
//! becomes a new tag, a key it already carries takes the request's value, and a key the
//! request does not name keeps the value it had — then answers the modelled 204 with no
//! body; the merged map is what every later read of that resource
//! (`ListTagsForResource` and the resource's own describe) reports, the resource itself is
//! otherwise untouched, a `Tags` map that breaks AWS's resource-tagging restrictions is
//! refused with `UnprocessableEntityException` (HTTP 422) attributed to `tags` with
//! nothing stored, and a `ResourceArn` naming no resource this service holds in the Region
//! the request is served in is answered `NotFoundException` (HTTP 404) with nothing stored
//! anywhere.
//!
//! Each test is a TABLE. The rows span the three resource types this service's ARNs name
//! (`smithy_query(query="digest")` — PackagingGroup, PackagingConfiguration, Asset) and
//! the ways an addition can relate to the stored map: the first tag of an untagged
//! resource, an empty addition, one new key, several new keys, a key that replaces a
//! stored one, a replacement beside an addition, the stored tags added again, keys
//! differing only in case, keys and values carrying spaces and punctuation, non-ASCII
//! keys and values, and an empty value. The refusal rows span each restriction, including
//! one that is broken only by the MERGED map rather than by the request's own.
//!
//! Every case drives real HTTP through the router the production binary builds. The
//! harness (`router`, `call`, `KINDS`, `tags_route`, `tag_map`, the creates) is the one
//! `tests/all/resource_tag_listing.rs` defines for the tag slice, and the tag-map builders
//! are the ones `tests/all/tag_restrictions.rs` defines for the restrictions.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_mediapackagevod_mediapackagevod::state as vod;

use crate::resource_tag_listing::{
    call, create_parent_group, create_with_tags, expected_map, router, tag_map, tags_route, Answer,
    KINDS, OTHER_REGION, PARENT_GROUP, REGION, SOURCE_ARN,
};
use crate::resource_tag_removal::listed_tags;
use crate::tag_restrictions::{errors_of, numbered_tags, one_tag, repeated, tags_of};

/// The modelled refusal of a resource this service does not hold
/// (`smithy_query(query="shape",shape="NotFoundException")` — `httpError 404`).
const NOT_FOUND: (u16, &str) = (404, "NotFoundException");

/// The modelled refusal of an invalid request parameter
/// (`smithy_query(query="shape",shape="UnprocessableEntityException")` — `httpError 422`).
const UNPROCESSABLE: (u16, &str) = (422, "UnprocessableEntityException");

/// The modelled success of `TagResource`: `@http` code 204, so no body at all
/// (`smithy_query(query="operation",operation="TagResource")`).
const NO_CONTENT: u16 = 204;

/// Add `tags` to `arn` in `region` and report what the call answered. `Tags` is a
/// structure member of the request body (`jsonName: tags`), so the map travels as the
/// body of a `POST /tags/{ResourceArn}`.
async fn tag(router: &MultiServiceRouter, region: &str, arn: &str, tags: &Value) -> Answer {
    call(
        router,
        region,
        "POST",
        &tags_route(arn),
        Some(json!({ "tags": tags })),
    )
    .await
}

/// Assert one successful `TagResource`: the modelled 204, and no body with it.
fn assert_no_content(answer: &Answer, case: &str) {
    assert_eq!(
        answer.status, NO_CONTENT,
        "case {case:?}: TagResource expected {NO_CONTENT}, got {} body {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.body,
        Value::Null,
        "case {case:?}: a 204 carries no body, got {}",
        answer.body
    );
}

/// One addition row: the tags the resource is created with, the map the tag request
/// carries, and the map that must be stored afterwards.
struct Row {
    case: &'static str,
    created: Option<Value>,
    add: Value,
    merged: Value,
}

/// The additions the table spans. `Tags` is a `__mapOf__string` carrying no declared
/// constraint (`smithy_query(query="shape",shape="TagResourceRequest")`), so every key and
/// value below is one the service accepts; the empty VALUE is admitted by AWS's tagging
/// restrictions too ("Values can be empty"), while an empty KEY is not and belongs to the
/// refusal table.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the first tag of a resource carrying none",
            created: None,
            add: json!({ "environment": "production" }),
            merged: json!({ "environment": "production" }),
        },
        Row {
            case: "a tag added to a resource created with an empty map",
            created: Some(json!({})),
            add: json!({ "environment": "production" }),
            merged: json!({ "environment": "production" }),
        },
        Row {
            case: "an empty addition adds nothing",
            created: Some(json!({ "environment": "production" })),
            add: json!({}),
            merged: json!({ "environment": "production" }),
        },
        Row {
            case: "one new key beside the stored ones",
            created: Some(json!({ "environment": "production", "team": "video" })),
            add: json!({ "tier": "gold" }),
            merged: json!({ "environment": "production", "team": "video", "tier": "gold" }),
        },
        Row {
            case: "several new keys at once",
            created: Some(json!({ "a": "1" })),
            add: json!({ "b": "2", "c": "3", "d": "4" }),
            merged: json!({ "a": "1", "b": "2", "c": "3", "d": "4" }),
        },
        Row {
            case: "a key the resource already carries takes the new value",
            created: Some(json!({ "environment": "production", "team": "video" })),
            add: json!({ "environment": "staging" }),
            merged: json!({ "environment": "staging", "team": "video" }),
        },
        Row {
            case: "one key replaced while another is added",
            created: Some(json!({ "environment": "production" })),
            add: json!({ "environment": "staging", "tier": "gold" }),
            merged: json!({ "environment": "staging", "tier": "gold" }),
        },
        Row {
            case: "the tags the resource already carries, added again",
            created: Some(json!({ "environment": "production", "team": "video" })),
            add: json!({ "environment": "production", "team": "video" }),
            merged: json!({ "environment": "production", "team": "video" }),
        },
        Row {
            case: "a key differing only in case is a different tag",
            created: Some(json!({ "environment": "production" })),
            add: json!({ "Environment": "staging" }),
            merged: json!({ "environment": "production", "Environment": "staging" }),
        },
        Row {
            case: "keys and values carrying spaces and punctuation",
            created: Some(json!({ "note": "renewed, 2026-01-01" })),
            add: json!({ "Cost Center": "video / vod" }),
            merged: json!({ "note": "renewed, 2026-01-01", "Cost Center": "video / vod" }),
        },
        Row {
            case: "non-ASCII keys and values",
            created: Some(json!({ "região": "são-paulo" })),
            add: json!({ "プロジェクト": "配信" }),
            merged: json!({ "região": "são-paulo", "プロジェクト": "配信" }),
        },
        Row {
            case: "an empty value",
            created: Some(json!({ "environment": "production" })),
            add: json!({ "note": "" }),
            merged: json!({ "environment": "production", "note": "" }),
        },
    ]
}

/// Tagging merges the named tags into the stored map and touches nothing else, for every
/// resource type and every shape of addition — and the merged map is what
/// `ListTagsForResource` and the resource's own describe both report afterwards.
#[tokio::test]
async fn tagging_merges_the_named_tags_into_the_stored_map() {
    let router = router();
    create_parent_group(&router, REGION).await;

    for kind in KINDS {
        for (index, row) in rows().into_iter().enumerate() {
            let case = format!("{}/{}", kind.name, row.case);
            let id = format!("{}-add-{index}", kind.arn_kind);
            let arn =
                create_with_tags(&router, REGION, kind, &id, row.created.as_ref(), &case).await;

            let answer = tag(&router, REGION, &arn, &row.add).await;
            assert_no_content(&answer, &case);

            let expected = expected_map(Some(&row.merged));
            assert_eq!(
                listed_tags(&router, REGION, &arn, &case).await,
                expected,
                "case {case:?}: after adding {} expected {expected:?}",
                row.add
            );

            // The one stored copy: the resource's own read reports the merged map too,
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

/// Tagging is idempotent: the same request repeated answers 204 again and leaves the same
/// tag map, for every resource type and whether the keys were already there.
#[tokio::test]
async fn repeating_a_tag_request_changes_nothing_further() {
    let router = router();
    create_parent_group(&router, REGION).await;
    let created = json!({ "environment": "production", "team": "video" });

    // Rows: keys the resource does not carry, a key it does, and a mixture.
    let additions: [(&str, Value, Value); 3] = [
        (
            "keys the resource does not carry",
            json!({ "tier": "gold" }),
            json!({ "environment": "production", "team": "video", "tier": "gold" }),
        ),
        (
            "a key the resource already carries",
            json!({ "environment": "staging" }),
            json!({ "environment": "staging", "team": "video" }),
        ),
        (
            "a replacement beside an addition",
            json!({ "team": "platform", "tier": "silver" }),
            json!({ "environment": "production", "team": "platform", "tier": "silver" }),
        ),
    ];

    for kind in KINDS {
        for (index, (row, add, merged)) in additions.iter().enumerate() {
            let case = format!("{}/{row}", kind.name);
            let id = format!("{}-repeat-{index}", kind.arn_kind);
            let arn = create_with_tags(&router, REGION, kind, &id, Some(&created), &case).await;
            let expected = expected_map(Some(merged));

            for attempt in 1..=3 {
                let answer = tag(&router, REGION, &arn, add).await;
                assert_no_content(&answer, &format!("{case} attempt {attempt}"));
                assert_eq!(
                    listed_tags(&router, REGION, &arn, &case).await,
                    expected,
                    "case {case:?} attempt {attempt}: expected {expected:?} after adding {add}"
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
/// were stored for a resource this service does not hold — and nothing is stored for the
/// resource it does hold.
#[tokio::test]
async fn tagging_an_arn_naming_no_resource_of_this_region_is_not_found() {
    let router = router();
    create_parent_group(&router, REGION).await;
    let account = engine::emulator_account_id();
    let held_tags = json!({ "environment": "production" });
    let held = create_with_tags(
        &router,
        REGION,
        KINDS[0],
        "vod-held-group",
        Some(&held_tags),
        "the resource the misses are compared against",
    )
    .await;

    // A resource of the SAME id exists in another Region, so a cross-Region tag that
    // answered 204 would have written that other Region's tags.
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
        let answer = tag(&router, REGION, &arn, &json!({ "tier": "gold" })).await;
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

    // Every refusal left the resource this service DOES hold exactly as it was.
    let expected = expected_map(Some(&held_tags));
    assert_eq!(
        listed_tags(&router, REGION, &held, "the held resource after every miss").await,
        expected,
        "a refused tag request must store nothing: expected {expected:?}"
    );
}

/// A tag request resolves only the resources of the Region it is served in: tagging the
/// resource of one Region leaves the same-named resource of another Region untouched.
#[tokio::test]
async fn tagging_in_one_region_leaves_another_regions_tags_alone() {
    let router = router();
    let tags = json!({ "environment": "production" });

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

    let arn_in =
        |region: &str| vod::resource_arn(vod::ARN_KIND_PACKAGING_GROUPS, region, PARENT_GROUP);
    let answer = tag(
        &router,
        REGION,
        &arn_in(REGION),
        &json!({ "environment": "staging", "tier": "gold" }),
    )
    .await;
    assert_no_content(&answer, "tag in the serving Region only");

    assert_eq!(
        listed_tags(&router, REGION, &arn_in(REGION), REGION).await,
        expected_map(Some(&json!({ "environment": "staging", "tier": "gold" }))),
        "the serving Region's group must carry exactly the merged map"
    );
    assert_eq!(
        listed_tags(&router, OTHER_REGION, &arn_in(OTHER_REGION), OTHER_REGION).await,
        expected_map(Some(&tags)),
        "the other Region's group of the same id must be untouched"
    );
}

/// One refusal row: the tags the resource is created with, and the addition that breaks a
/// restriction.
struct Refused {
    case: String,
    created: Value,
    add: Value,
}

/// The additions that break AWS's resource-tagging restrictions, spanning each one — and
/// including the row that is inside the restrictions as a REQUEST but outside them as a
/// resource, because the merged map would carry more tags than the limit allows.
fn refused_rows() -> Vec<Refused> {
    let reserved = vod::RESERVED_TAG_KEY_PREFIX;
    let one = json!({ "environment": "production" });
    vec![
        Refused {
            case: "a reserved key alone".to_string(),
            created: one.clone(),
            add: one_tag(format!("{reserved}owner"), "platform".to_string()),
        },
        Refused {
            case: "a reserved key beside a well-formed one".to_string(),
            created: one.clone(),
            add: tags_of(&[
                ("tier".to_string(), "gold".to_string()),
                (format!("{reserved}owner"), "platform".to_string()),
            ]),
        },
        Refused {
            case: "a key one character past its limit".to_string(),
            created: one.clone(),
            add: one_tag(repeated("k", vod::MAX_TAG_KEY_CHARS + 1), "v".to_string()),
        },
        Refused {
            case: "a multi-byte key one character past its limit".to_string(),
            created: one.clone(),
            add: one_tag(repeated("é", vod::MAX_TAG_KEY_CHARS + 1), "v".to_string()),
        },
        Refused {
            case: "a value one character past its limit".to_string(),
            created: one.clone(),
            add: one_tag(
                "note".to_string(),
                repeated("v", vod::MAX_TAG_VALUE_CHARS + 1),
            ),
        },
        Refused {
            case: format!("one request past the {} tags allowed", vod::MAX_TAGS),
            created: one.clone(),
            add: numbered_tags(vod::MAX_TAGS + 1),
        },
        Refused {
            case: "an addition that takes the RESOURCE one tag past the limit".to_string(),
            created: numbered_tags(vod::MAX_TAGS),
            add: one_tag("extra".to_string(), "one".to_string()),
        },
        Refused {
            case: "an addition that takes the RESOURCE far past the limit".to_string(),
            created: numbered_tags(vod::MAX_TAGS),
            add: tags_of(
                &(0..vod::MAX_TAGS)
                    .map(|index| (format!("extra{index}"), "one".to_string()))
                    .collect::<Vec<_>>(),
            ),
        },
    ]
}

/// An addition that breaks the restrictions is refused with the modelled 422 attributed
/// to `tags`, and NOTHING is stored: the resource still reports exactly the tags it
/// carried before the refused call.
#[tokio::test]
async fn an_addition_outside_the_restrictions_is_refused_and_stores_nothing() {
    let router = router();
    create_parent_group(&router, REGION).await;

    for kind in KINDS {
        for (index, row) in refused_rows().into_iter().enumerate() {
            let case = format!("{}/{}", kind.name, row.case);
            let id = format!("{}-refused-{index}", kind.arn_kind);
            let arn =
                create_with_tags(&router, REGION, kind, &id, Some(&row.created), &case).await;

            let answer = tag(&router, REGION, &arn, &row.add).await;
            assert_eq!(
                (answer.status, answer.errortype.as_str()),
                UNPROCESSABLE,
                "case {case:?}: expected {UNPROCESSABLE:?}, got {} {:?} body {}",
                answer.status,
                answer.errortype,
                answer.body
            );
            let errors = errors_of(&answer.body);
            assert!(
                errors.get("tags").is_some_and(|phrases| !phrases.is_empty()),
                "case {case:?}: the refusal must attribute a reason to `tags`, got {errors:?}"
            );

            let expected = expected_map(Some(&row.created));
            assert_eq!(
                listed_tags(&router, REGION, &arn, &case).await,
                expected,
                "case {case:?}: a refused addition must store nothing, expected {expected:?}"
            );
        }
    }
}

/// An addition INSIDE every restriction — including each published bound exactly, and a
/// replacement that leaves a resource already at the tag limit still at it — is accepted
/// and round-trips.
#[tokio::test]
async fn an_addition_inside_the_restrictions_is_accepted_and_round_trips() {
    let router = router();
    create_parent_group(&router, REGION).await;

    // Rows: (case, the tags created with, the addition, the merged map).
    let at_the_limit = numbered_tags(vod::MAX_TAGS);
    let mut replaced = at_the_limit.clone();
    replaced["key0"] = Value::String("replaced".to_string());
    let rows: Vec<(String, Value, Value, Value)> = vec![
        (
            format!("filling a resource to the {} tags allowed", vod::MAX_TAGS),
            json!({}),
            at_the_limit.clone(),
            at_the_limit.clone(),
        ),
        (
            "replacing a key of a resource already at the tag limit".to_string(),
            at_the_limit.clone(),
            one_tag("key0".to_string(), "replaced".to_string()),
            replaced,
        ),
        (
            "a key exactly at its limit".to_string(),
            json!({}),
            one_tag(repeated("k", vod::MAX_TAG_KEY_CHARS), "v".to_string()),
            one_tag(repeated("k", vod::MAX_TAG_KEY_CHARS), "v".to_string()),
        ),
        (
            "a multi-byte key exactly at its limit".to_string(),
            json!({}),
            one_tag(repeated("é", vod::MAX_TAG_KEY_CHARS), "v".to_string()),
            one_tag(repeated("é", vod::MAX_TAG_KEY_CHARS), "v".to_string()),
        ),
        (
            "a value exactly at its limit".to_string(),
            json!({}),
            one_tag("note".to_string(), repeated("v", vod::MAX_TAG_VALUE_CHARS)),
            one_tag("note".to_string(), repeated("v", vod::MAX_TAG_VALUE_CHARS)),
        ),
        (
            "a key merely CARRYING the reserved prefix, not starting with it".to_string(),
            json!({}),
            one_tag(
                format!("team-{}owner", vod::RESERVED_TAG_KEY_PREFIX),
                "platform".to_string(),
            ),
            one_tag(
                format!("team-{}owner", vod::RESERVED_TAG_KEY_PREFIX),
                "platform".to_string(),
            ),
        ),
    ];

    for kind in KINDS {
        for (index, (row, created, add, merged)) in rows.iter().enumerate() {
            let case = format!("{}/{row}", kind.name);
            let id = format!("{}-accepted-{index}", kind.arn_kind);
            let arn = create_with_tags(&router, REGION, kind, &id, Some(created), &case).await;

            let answer = tag(&router, REGION, &arn, add).await;
            assert_no_content(&answer, &case);

            let expected = expected_map(Some(merged));
            assert_eq!(
                listed_tags(&router, REGION, &arn, &case).await,
                expected,
                "case {case:?}: expected the merged map of {} entries",
                expected.len()
            );
        }
    }
}
