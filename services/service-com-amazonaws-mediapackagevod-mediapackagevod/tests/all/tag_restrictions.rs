// SPDX-License-Identifier: CC0-1.0

//! AWS's resource-tagging restrictions on the tag maps this service accepts.
//!
//! The rule under test, stated once: a caller-supplied `Tags` map is refused with
//! `UnprocessableEntityException` (HTTP 422) attributed to the member it arrived on,
//! and NOTHING is created or changed, when it breaks AWS's universal resource-tagging
//! restrictions — more than [`vod::MAX_TAGS`] entries, a key longer than
//! [`vod::MAX_TAG_KEY_CHARS`] characters, a value longer than
//! [`vod::MAX_TAG_VALUE_CHARS`] characters, or a key carrying the reserved
//! [`vod::RESERVED_TAG_KEY_PREFIX`] only AWS may set (which a caller may no more delete
//! than create). A map inside every one of those bounds is accepted and round-trips.
//!
//! Each test is a TABLE whose rows differ along what the rule generalises over: which
//! restriction is broken, by how much (at the published bound, one past it, far past
//! it), whether the offending entry travels alone or beside valid ones, and whether the
//! characters counted are single- or multi-byte — the counts are CHARACTERS, not bytes.
//! Every row runs against all three resource types whose create accepts a tag map
//! (PackagingGroup, PackagingConfiguration, Asset), so the rule is pinned rather than
//! one recorded request.
//!
//! The bounds themselves are read from the constants the service publishes, so a row
//! cannot drift from the rule the handlers enforce.
//!
//! Every case drives real HTTP through the router the production binary builds.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use service_com_amazonaws_mediapackagevod_mediapackagevod::state as vod;

use crate::resource_tag_listing::{
    call, create_parent_group, expected_map, router, tag_map, tags_route, KINDS, REGION,
};
use crate::resource_tag_removal::untag_route;

/// The modelled refusal of an invalid request parameter: error code and `httpError`
/// (`smithy_query(query="shape",shape="UnprocessableEntityException")` — `errorKind
/// client`, `httpError 422`).
const UNPROCESSABLE: (u16, &str) = (422, "UnprocessableEntityException");

/// The modelled refusal of a resource this service does not hold
/// (`smithy_query(query="shape",shape="NotFoundException")` — `httpError 404`).
const NOT_FOUND: (u16, &str) = (404, "NotFoundException");

/// The modelled success of `UntagResource`: `@http` code 204, so no body at all.
const NO_CONTENT: u16 = 204;

/// A tag map of `count` distinct, well-formed entries — the count dimension of the
/// rule, with nothing else about the map at fault.
fn numbered_tags(count: usize) -> Value {
    let map: serde_json::Map<String, Value> = (0..count)
        .map(|index| (format!("key{index}"), Value::String(format!("value{index}"))))
        .collect();
    Value::Object(map)
}

/// `text` repeated until it is `count` CHARACTERS long. `text` is one character, so a
/// multi-byte one gives a string whose byte length is a multiple of its char length —
/// which is what separates counting characters from counting bytes.
fn repeated(text: &str, count: usize) -> String {
    text.repeat(count)
}

/// A tag map built from `(key, value)` pairs, so a row can name a key the `json!`
/// macro could not take as a literal.
fn tags_of(pairs: &[(String, String)]) -> Value {
    Value::Object(
        pairs
            .iter()
            .map(|(key, value)| (key.clone(), Value::String(value.clone())))
            .collect(),
    )
}

/// A one-entry tag map with the given key and value.
fn one_tag(key: String, value: String) -> Value {
    tags_of(&[(key, value)])
}

/// One row of the refusal table: the case, the tag map the request carries, and the
/// member the refusal must be attributed to.
struct Refused {
    case: String,
    tags: Value,
}

/// The tag maps that break the restrictions, spanning every restriction and, for each
/// bounded one, both one character past the bound and far past it.
fn refused_rows() -> Vec<Refused> {
    let reserved = vod::RESERVED_TAG_KEY_PREFIX;
    vec![
        Refused {
            case: "a reserved key alone".to_string(),
            tags: one_tag(format!("{reserved}owner"), "platform".to_string()),
        },
        Refused {
            case: "a reserved key beside well-formed ones".to_string(),
            tags: tags_of(&[
                ("env".to_string(), "test".to_string()),
                (
                    format!("{reserved}cloudformation:stack-name"),
                    "vod-stack".to_string(),
                ),
                ("team".to_string(), "video".to_string()),
            ]),
        },
        Refused {
            case: "a key that is nothing but the reserved prefix".to_string(),
            tags: one_tag(reserved.to_string(), "x".to_string()),
        },
        Refused {
            case: format!("one tag past the {} the limit allows", vod::MAX_TAGS),
            tags: numbered_tags(vod::MAX_TAGS + 1),
        },
        Refused {
            case: "far past the tag-count limit".to_string(),
            tags: numbered_tags(vod::MAX_TAGS * 2),
        },
        Refused {
            case: "a key one character past its limit".to_string(),
            tags: one_tag(
                repeated("k", vod::MAX_TAG_KEY_CHARS + 1),
                "v".to_string(),
            ),
        },
        Refused {
            case: "a key far past its limit".to_string(),
            tags: one_tag(
                repeated("k", vod::MAX_TAG_KEY_CHARS * 2),
                "v".to_string(),
            ),
        },
        Refused {
            case: "a multi-byte key one character past its limit".to_string(),
            tags: one_tag(
                repeated("é", vod::MAX_TAG_KEY_CHARS + 1),
                "v".to_string(),
            ),
        },
        Refused {
            case: "a value one character past its limit".to_string(),
            tags: one_tag(
                "env".to_string(),
                repeated("v", vod::MAX_TAG_VALUE_CHARS + 1),
            ),
        },
        Refused {
            case: "a value far past its limit".to_string(),
            tags: one_tag(
                "env".to_string(),
                repeated("v", vod::MAX_TAG_VALUE_CHARS * 2),
            ),
        },
        Refused {
            case: "a multi-byte value one character past its limit".to_string(),
            tags: one_tag(
                "env".to_string(),
                repeated("é", vod::MAX_TAG_VALUE_CHARS + 1),
            ),
        },
        Refused {
            case: "a reserved key and an over-long value in one request".to_string(),
            tags: tags_of(&[
                (format!("{reserved}owner"), "platform".to_string()),
                (
                    "env".to_string(),
                    repeated("v", vod::MAX_TAG_VALUE_CHARS + 1),
                ),
            ]),
        },
    ]
}

/// The tag maps that are INSIDE every restriction, including each published bound
/// exactly — accepted, and reported back unchanged.
fn accepted_rows() -> Vec<(String, Value)> {
    let reserved = vod::RESERVED_TAG_KEY_PREFIX;
    vec![
        (
            format!("exactly the {} tags the limit allows", vod::MAX_TAGS),
            numbered_tags(vod::MAX_TAGS),
        ),
        (
            "a key exactly at its limit".to_string(),
            one_tag(repeated("k", vod::MAX_TAG_KEY_CHARS), "v".to_string()),
        ),
        (
            "a multi-byte key exactly at its limit".to_string(),
            one_tag(repeated("é", vod::MAX_TAG_KEY_CHARS), "v".to_string()),
        ),
        (
            "a value exactly at its limit".to_string(),
            one_tag("env".to_string(), repeated("v", vod::MAX_TAG_VALUE_CHARS)),
        ),
        (
            "a multi-byte value exactly at its limit".to_string(),
            one_tag("env".to_string(), repeated("é", vod::MAX_TAG_VALUE_CHARS)),
        ),
        (
            "a key merely CARRYING the reserved prefix, not starting with it".to_string(),
            one_tag(format!("team-{reserved}owner"), "platform".to_string()),
        ),
        (
            "a key whose first characters only resemble the prefix".to_string(),
            one_tag("awsome".to_string(), "yes".to_string()),
        ),
    ]
}

/// The `errors` attribution of a refusal body, as the member-keyed map it is.
fn errors_of(body: &Value) -> BTreeMap<String, Vec<String>> {
    body.get("errors")
        .and_then(Value::as_object)
        .map(|errors| {
            errors
                .iter()
                .map(|(member, phrases)| {
                    let phrases = phrases
                        .as_array()
                        .map(|phrases| {
                            phrases
                                .iter()
                                .filter_map(Value::as_str)
                                .map(str::to_string)
                                .collect()
                        })
                        .unwrap_or_default();
                    (member.clone(), phrases)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A create carrying a tag map that breaks the restrictions is refused with the
/// modelled 422 attributed to `tags`, and NO resource is created: the id still reads
/// back as not found, and a later create of the same id inherits none of the refused
/// tags.
#[tokio::test]
async fn a_create_carrying_a_tag_map_outside_the_restrictions_is_refused_and_creates_nothing() {
    let router = router();
    create_parent_group(&router, REGION).await;

    for kind in KINDS {
        for (index, row) in refused_rows().into_iter().enumerate() {
            let case = format!("{}/{}", kind.name, row.case);
            let id = format!("refused-{}-{index}", kind.arn_kind);
            let answer = call(
                &router,
                REGION,
                "POST",
                kind.create_route,
                Some(kind.create_body(&id, Some(&row.tags))),
            )
            .await;
            assert_eq!(
                (answer.status, answer.errortype.as_str()),
                UNPROCESSABLE,
                "case {case:?}: expected {UNPROCESSABLE:?}, got {} {:?} body {}",
                answer.status,
                answer.errortype,
                answer.body
            );
            let errors = errors_of(&answer.body);
            let phrases = errors.get("tags").cloned().unwrap_or_default();
            assert!(
                !phrases.is_empty(),
                "case {case:?}: the refusal must attribute at least one reason to `tags`, \
                 got {errors:?}"
            );
            assert!(
                answer
                    .body
                    .get("message")
                    .and_then(Value::as_str)
                    .is_some_and(|message| !message.is_empty()),
                "case {case:?}: the refusal must carry a message, got {}",
                answer.body
            );

            // Nothing was created: the read of that id is the same miss it was before
            // the refused create.
            let described = call(&router, REGION, "GET", &kind.describe_route(&id), None).await;
            assert_eq!(
                (described.status, described.errortype.as_str()),
                NOT_FOUND,
                "case {case:?}: after the refusal the id must not exist, got {} {:?} body {}",
                described.status,
                described.errortype,
                described.body
            );

            // And no tag was stored under the ARN the refused create would have used:
            // the same id created cleanly afterwards carries only its own tags.
            let kept = json!({ "env": "kept" });
            let recreated = call(
                &router,
                REGION,
                "POST",
                kind.create_route,
                Some(kind.create_body(&id, Some(&kept))),
            )
            .await;
            assert_eq!(
                recreated.status,
                kind.created_status,
                "case {case:?}: a clean create of the same id must succeed, got {} body {}",
                recreated.status,
                recreated.body
            );
            assert_eq!(
                tag_map(&recreated.body),
                expected_map(Some(&kept)),
                "case {case:?}: the clean create must carry only its own tags, got {}",
                recreated.body["tags"]
            );
        }
    }
}

/// A tag map inside every restriction — including each published bound exactly — is
/// accepted and round-trips: the create reports it and the resource's own describe
/// reports the same map.
#[tokio::test]
async fn a_create_carrying_a_tag_map_inside_the_restrictions_is_accepted_and_round_trips() {
    let router = router();
    create_parent_group(&router, REGION).await;

    for kind in KINDS {
        for (index, (row, tags)) in accepted_rows().into_iter().enumerate() {
            let case = format!("{}/{row}", kind.name);
            let id = format!("accepted-{}-{index}", kind.arn_kind);
            let created = call(
                &router,
                REGION,
                "POST",
                kind.create_route,
                Some(kind.create_body(&id, Some(&tags))),
            )
            .await;
            assert_eq!(
                created.status,
                kind.created_status,
                "case {case:?}: expected {}, got {} body {}",
                kind.created_status,
                created.status,
                created.body
            );
            let expected = expected_map(Some(&tags));
            assert_eq!(
                tag_map(&created.body),
                expected,
                "case {case:?}: the create must report the tags it was given, got {}",
                created.body["tags"]
            );
            let described = call(&router, REGION, "GET", &kind.describe_route(&id), None).await;
            assert_eq!(
                described.status, 200,
                "case {case:?}: the read must succeed, got {} body {}",
                described.status, described.body
            );
            assert_eq!(
                tag_map(&described.body),
                expected,
                "case {case:?}: the read must report the stored tags, got {}",
                described.body["tags"]
            );
        }
    }
}

/// A reserved key may no more be DELETED than created: `UntagResource` naming one is
/// refused with the modelled 422 attributed to `tagKeys` and removes nothing, while the
/// same request naming a well-formed key still removes exactly that key.
#[tokio::test]
async fn untagging_a_reserved_key_is_refused_and_removes_nothing() {
    let router = router();
    create_parent_group(&router, REGION).await;
    let reserved = vod::RESERVED_TAG_KEY_PREFIX;
    let held = json!({ "env": "production", "team": "video" });

    // The ways a reserved key can reach `tagKeys`: alone, beside a key the resource
    // really carries, and as the bare prefix.
    let reserved_rows: Vec<(String, Vec<String>)> = vec![
        (
            "the reserved key alone".to_string(),
            vec![format!("{reserved}owner")],
        ),
        (
            "a reserved key beside one the resource carries".to_string(),
            vec!["env".to_string(), format!("{reserved}owner")],
        ),
        (
            "the bare reserved prefix".to_string(),
            vec![reserved.to_string()],
        ),
    ];

    for kind in KINDS {
        for (index, (row, keys)) in reserved_rows.iter().enumerate() {
            let case = format!("{}/{row}", kind.name);
            let id = format!("untag-{}-{index}", kind.arn_kind);
            let created = call(
                &router,
                REGION,
                "POST",
                kind.create_route,
                Some(kind.create_body(&id, Some(&held))),
            )
            .await;
            assert_eq!(
                created.status,
                kind.created_status,
                "case {case:?}: the setup create must succeed, got {} body {}",
                created.status,
                created.body
            );
            let arn = created.body["arn"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: the create reported no Arn"))
                .to_string();

            let borrowed: Vec<&str> = keys.iter().map(String::as_str).collect();
            let refused =
                call(&router, REGION, "DELETE", &untag_route(&arn, &borrowed), None).await;
            assert_eq!(
                (refused.status, refused.errortype.as_str()),
                UNPROCESSABLE,
                "case {case:?}: expected {UNPROCESSABLE:?}, got {} {:?} body {}",
                refused.status,
                refused.errortype,
                refused.body
            );
            let errors = errors_of(&refused.body);
            assert!(
                errors.get("tagKeys").is_some_and(|phrases| !phrases.is_empty()),
                "case {case:?}: the refusal must attribute a reason to `tagKeys`, got {errors:?}"
            );

            // Nothing was removed: every tag the resource was created with is still
            // reported, including the one the refused request also named.
            let listed = call(&router, REGION, "GET", &tags_route(&arn), None).await;
            assert_eq!(
                tag_map(&listed.body),
                expected_map(Some(&held)),
                "case {case:?}: the refused removal must leave every tag in place, got {}",
                listed.body["tags"]
            );

            // A well-formed key is still removed, so the refusal is about the reserved
            // key and not about untagging at all.
            let accepted = call(&router, REGION, "DELETE", &untag_route(&arn, &["env"]), None).await;
            assert_eq!(
                accepted.status, NO_CONTENT,
                "case {case:?}: removing a well-formed key must answer {NO_CONTENT}, got {} body {}",
                accepted.status, accepted.body
            );
            let remaining = call(&router, REGION, "GET", &tags_route(&arn), None).await;
            assert_eq!(
                tag_map(&remaining.body),
                expected_map(Some(&json!({ "team": "video" }))),
                "case {case:?}: only the named key may be gone, got {}",
                remaining.body["tags"]
            );
        }
    }
}
