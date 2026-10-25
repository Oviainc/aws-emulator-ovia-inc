// SPDX-License-Identifier: CC0-1.0

//! A tag key is bounded at BOTH ends, so the empty string is not a tag key.
//!
//! The rule under test, stated once: AWS's universal resource-tagging restrictions bound
//! a tag key at [`vod::MIN_TAG_KEY_CHARS`] to [`vod::MAX_TAG_KEY_CHARS`] Unicode
//! characters, so a request naming a key outside that bound — the empty string among
//! them — is refused with `UnprocessableEntityException` (HTTP 422) attributed to the
//! member the key arrived on, changing nothing; a tag VALUE carries no such minimum, so
//! an empty value is accepted. The bound holds wherever a key is named: on the creates
//! that accept a `Tags` map (judged as `tags`) and on `UntagResource`, which names keys
//! to remove (judged as `tagKeys`) — removing a key no tag could carry is refused, not
//! reported as an idempotent removal that removed nothing.
//!
//! Each test is a TABLE whose rows differ along what the rule generalises over: where the
//! out-of-bound key sits in the request (alone, before or after well-formed keys,
//! repeated, beside a key refused for another reason), which published bound is at stake
//! (one character inside the minimum, the minimum exactly, the maximum exactly, one past
//! it), and whether the characters counted are single- or multi-byte. Every row runs
//! against all three resource types whose create accepts a tag map (PackagingGroup,
//! PackagingConfiguration, Asset).
//!
//! The bounds come from the constants the service publishes
//! (`vod::MIN_TAG_KEY_CHARS` / `vod::MAX_TAG_KEY_CHARS`, which cite AWS's tagging
//! restrictions), so a row cannot drift from the rule the handlers enforce.
//!
//! Every case drives real HTTP through the router the production binary builds. The
//! harness is the one `tests/all/resource_tag_listing.rs` defines for the tag slice.

use serde_json::{json, Value};

use service_com_amazonaws_mediapackagevod_mediapackagevod::state as vod;

use crate::resource_tag_listing::{
    call, create_parent_group, expected_map, router, tag_map, tags_route, KINDS, REGION,
};
use crate::resource_tag_removal::untag_route;

/// The modelled refusal of an invalid request parameter: status and error code
/// (`smithy_query(query="shape",shape="UnprocessableEntityException")` — `errorKind
/// client`, `httpError 422`).
const UNPROCESSABLE: (u16, &str) = (422, "UnprocessableEntityException");

/// The modelled refusal of a resource this service does not hold
/// (`smithy_query(query="shape",shape="NotFoundException")` — `httpError 404`).
const NOT_FOUND: (u16, &str) = (404, "NotFoundException");

/// The modelled success of `UntagResource`: `@http` code 204, so no body at all.
const NO_CONTENT: u16 = 204;

/// The key no tag can carry: zero characters is below [`vod::MIN_TAG_KEY_CHARS`].
const EMPTY_KEY: &str = "";

/// `text` repeated until it is `count` CHARACTERS long, so a multi-byte `text` gives a
/// string whose byte length is a multiple of its character length.
fn repeated(text: &str, count: usize) -> String {
    text.repeat(count)
}

/// The phrases a refusal attributed to `member`, or an empty list when it attributed
/// none.
fn phrases_for(body: &Value, member: &str) -> Vec<String> {
    body.get("errors")
        .and_then(Value::as_object)
        .and_then(|errors| errors.get(member))
        .and_then(Value::as_array)
        .map(|phrases| {
            phrases
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Assert one refusal of an out-of-bound tag key: the modelled 422, a reason attributed
/// to `member` naming the bound that was broken, and a message.
fn assert_refused_key_length(
    answer: &crate::resource_tag_listing::Answer,
    member: &str,
    bound_phrase: &str,
    case: &str,
) {
    assert_eq!(
        (answer.status, answer.errortype.as_str()),
        UNPROCESSABLE,
        "case {case:?}: expected {UNPROCESSABLE:?}, got {} {:?} body {}",
        answer.status,
        answer.errortype,
        answer.body
    );
    let phrases = phrases_for(&answer.body, member);
    assert!(
        phrases
            .iter()
            .any(|phrase| phrase.contains(bound_phrase)),
        "case {case:?}: the refusal must attribute {bound_phrase:?} to `{member}`, got {:?}",
        phrases
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
}

/// The phrase a refusal of a key SHORTER than the minimum carries, built from the
/// published bound.
fn below_minimum_phrase() -> String {
    format!("at least {} character", vod::MIN_TAG_KEY_CHARS)
}

/// The phrase a refusal of a key LONGER than the maximum carries, built from the
/// published bound.
fn above_maximum_phrase() -> String {
    format!("at most {} characters", vod::MAX_TAG_KEY_CHARS)
}

/// One `UntagResource` row: the case, and the `tagKeys` the request names.
struct Removal {
    case: &'static str,
    keys: Vec<String>,
}

/// The ways a key below the minimum can reach `tagKeys`: alone, on either side of a key
/// the resource really carries, repeated, and beside a key refused for the OTHER reason
/// a key can be refused.
fn refused_removals() -> Vec<Removal> {
    vec![
        Removal {
            case: "the empty key alone",
            keys: vec![EMPTY_KEY.to_string()],
        },
        Removal {
            case: "the empty key before one the resource carries",
            keys: vec![EMPTY_KEY.to_string(), "environment".to_string()],
        },
        Removal {
            case: "the empty key after one the resource carries",
            keys: vec!["environment".to_string(), EMPTY_KEY.to_string()],
        },
        Removal {
            case: "the empty key twice in one request",
            keys: vec![EMPTY_KEY.to_string(), EMPTY_KEY.to_string()],
        },
        Removal {
            case: "the empty key beside a reserved one",
            keys: vec![
                EMPTY_KEY.to_string(),
                format!("{}owner", vod::RESERVED_TAG_KEY_PREFIX),
            ],
        },
        Removal {
            case: "the empty key beside one the resource does not carry",
            keys: vec![EMPTY_KEY.to_string(), "never-applied".to_string()],
        },
    ]
}

/// `UntagResource` naming a key outside the published bound is refused with the modelled
/// 422 attributed to `tagKeys` and removes NOTHING — while the same resource's
/// well-formed keys are still removable afterwards, so the refusal is about the key and
/// not about untagging.
#[tokio::test]
async fn untagging_a_key_outside_the_published_bound_is_refused_and_removes_nothing() {
    let router = router();
    create_parent_group(&router, REGION).await;
    let held = json!({ "environment": "production", "team": "video" });

    // The rows about the MAXIMUM end of the same bound travel here too: one key past it,
    // judged on the same member by the same check.
    let over_long = repeated("k", vod::MAX_TAG_KEY_CHARS + 1);

    for kind in KINDS {
        for (index, row) in refused_removals().into_iter().enumerate() {
            let case = format!("{}/{}", kind.name, row.case);
            let id = format!("short-key-untag-{}-{index}", kind.arn_kind);
            let arn = crate::resource_tag_listing::create_with_tags(
                &router,
                REGION,
                kind,
                &id,
                Some(&held),
                &case,
            )
            .await;

            let borrowed: Vec<&str> = row.keys.iter().map(String::as_str).collect();
            let refused =
                call(&router, REGION, "DELETE", &untag_route(&arn, &borrowed), None).await;
            assert_refused_key_length(&refused, "tagKeys", &below_minimum_phrase(), &case);

            // Nothing was removed: every tag the resource was created with is still
            // reported, including any the refused request also named.
            let listed = call(&router, REGION, "GET", &tags_route(&arn), None).await;
            assert_eq!(
                tag_map(&listed.body),
                expected_map(Some(&held)),
                "case {case:?}: the refused removal must leave every tag in place, got {}",
                listed.body["tags"]
            );

            // The other end of the same bound, on the same resource.
            let too_long = call(
                &router,
                REGION,
                "DELETE",
                &untag_route(&arn, &[over_long.as_str()]),
                None,
            )
            .await;
            assert_refused_key_length(
                &too_long,
                "tagKeys",
                &above_maximum_phrase(),
                &format!("{case} (a key past the maximum)"),
            );

            // A key INSIDE the bound is still removed, and only that key.
            let accepted =
                call(&router, REGION, "DELETE", &untag_route(&arn, &["environment"]), None).await;
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

/// One create row: the case, and the `Tags` map the request carries.
struct Created {
    case: &'static str,
    tags: Value,
}

/// A tag map built from `(key, value)` pairs, so a row can name a key the `json!` macro
/// could not take as a literal.
fn tags_of(pairs: &[(String, String)]) -> Value {
    Value::Object(
        pairs
            .iter()
            .map(|(key, value)| (key.clone(), Value::String(value.clone())))
            .collect(),
    )
}

/// A one-entry tag map, for the rows that need only the one key.
fn one_tag(key: &str, value: &str) -> Value {
    tags_of(&[(key.to_string(), value.to_string())])
}

/// The ways a key below the minimum can reach a `Tags` map on a create.
fn refused_creates() -> Vec<Created> {
    vec![
        Created {
            case: "the empty key as the only tag",
            tags: one_tag(EMPTY_KEY, "v"),
        },
        Created {
            case: "the empty key beside well-formed tags",
            tags: tags_of(&[
                ("environment".to_string(), "production".to_string()),
                (EMPTY_KEY.to_string(), "v".to_string()),
                ("team".to_string(), "video".to_string()),
            ]),
        },
        Created {
            case: "the empty key with an empty value",
            tags: one_tag(EMPTY_KEY, EMPTY_KEY),
        },
    ]
}

/// A create whose `Tags` map names a key outside the published bound is refused with the
/// modelled 422 attributed to `tags`, and NOTHING is created: the id still reads back as
/// not found, so no tag whose key is the empty string can ever be reported.
#[tokio::test]
async fn a_create_naming_a_key_outside_the_published_bound_creates_nothing() {
    let router = router();
    create_parent_group(&router, REGION).await;

    for kind in KINDS {
        for (index, row) in refused_creates().into_iter().enumerate() {
            let case = format!("{}/{}", kind.name, row.case);
            let id = format!("short-key-create-{}-{index}", kind.arn_kind);
            let refused = call(
                &router,
                REGION,
                "POST",
                kind.create_route,
                Some(kind.create_body(&id, Some(&row.tags))),
            )
            .await;
            assert_refused_key_length(&refused, "tags", &below_minimum_phrase(), &case);

            let described = call(&router, REGION, "GET", &kind.describe_route(&id), None).await;
            assert_eq!(
                (described.status, described.errortype.as_str()),
                NOT_FOUND,
                "case {case:?}: after the refusal the id must not exist, got {} {:?} body {}",
                described.status,
                described.errortype,
                described.body
            );
        }
    }
}

/// A key exactly AT the minimum is accepted, and so is an empty VALUE: the minimum bounds
/// the key only. Each accepted map round-trips through `ListTagsForResource` and the
/// resource's own describe, and its shortest key is removable.
#[tokio::test]
async fn a_key_at_the_minimum_and_an_empty_value_are_accepted_and_round_trip() {
    let router = router();
    create_parent_group(&router, REGION).await;

    // Rows at and just inside the published bound. `é` is multi-byte, so a row whose key
    // is one CHARACTER long is not one BYTE long.
    let minimum = vod::MIN_TAG_KEY_CHARS;
    let shortest_ascii = repeated("e", minimum);
    let shortest_multibyte = repeated("é", minimum);
    let longest = repeated("k", vod::MAX_TAG_KEY_CHARS);
    let accepted: Vec<(String, Value, String)> = vec![
        (
            format!("an ASCII key of exactly {minimum} character"),
            one_tag(&shortest_ascii, "production"),
            shortest_ascii.clone(),
        ),
        (
            format!("a multi-byte key of exactly {minimum} character"),
            one_tag(&shortest_multibyte, "produção"),
            shortest_multibyte.clone(),
        ),
        (
            "a well-formed key whose VALUE is empty".to_string(),
            one_tag("environment", EMPTY_KEY),
            "environment".to_string(),
        ),
        (
            "a shortest key beside a longest one".to_string(),
            tags_of(&[
                (shortest_ascii.clone(), "production".to_string()),
                (longest.clone(), "v".to_string()),
            ]),
            shortest_ascii.clone(),
        ),
    ];

    for kind in KINDS {
        for (index, (row, tags, removable)) in accepted.iter().enumerate() {
            let case = format!("{}/{row}", kind.name);
            let id = format!("min-key-{}-{index}", kind.arn_kind);
            let arn = crate::resource_tag_listing::create_with_tags(
                &router, REGION, kind, &id, Some(tags), &case,
            )
            .await;

            let expected = expected_map(Some(tags));
            let listed = call(&router, REGION, "GET", &tags_route(&arn), None).await;
            assert_eq!(
                tag_map(&listed.body),
                expected,
                "case {case:?}: ListTagsForResource must report the stored tags, got {}",
                listed.body
            );
            let described = call(&router, REGION, "GET", &kind.describe_route(&id), None).await;
            assert_eq!(
                (described.status, tag_map(&described.body)),
                (200, expected.clone()),
                "case {case:?}: the describe must report the same tags, got {} body {}",
                described.status,
                described.body
            );

            // A key inside the bound is removable however short it is.
            let removed = call(
                &router,
                REGION,
                "DELETE",
                &untag_route(&arn, &[removable.as_str()]),
                None,
            )
            .await;
            assert_eq!(
                removed.status, NO_CONTENT,
                "case {case:?}: removing {removable:?} must answer {NO_CONTENT}, got {} body {}",
                removed.status, removed.body
            );
            let remaining = call(&router, REGION, "GET", &tags_route(&arn), None).await;
            let mut without = expected.clone();
            without.remove(removable.as_str());
            assert_eq!(
                tag_map(&remaining.body),
                without,
                "case {case:?}: only {removable:?} may be gone, got {}",
                remaining.body["tags"]
            );
        }
    }
}
