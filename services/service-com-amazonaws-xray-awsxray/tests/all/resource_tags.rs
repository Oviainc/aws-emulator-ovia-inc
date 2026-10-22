// SPDX-License-Identifier: CC0-1.0

//! Tagging an X-Ray resource end to end over the wire: `TagResource`,
//! `UntagResource` and `ListTagsForResource`.
//!
//! Every case drives the same front door the production router does (the
//! harness is `xray_wire::Xray`, reused rather than rebuilt), so
//! the assertions are about HTTP responses, not about internal state. The tests
//! are tables, and each row differs along what the rule it checks generalises
//! over — the number of tags, the character classes a key or a value carries,
//! the published size bounds, the overlap between what is stored and what the
//! request names, the count of tags relative to a page — because the rules are:
//!
//! * a tag set lives ON the resource its ARN names and is answered from that
//!   stored record, whichever operation wrote it (`CreateGroup` or
//!   `TagResource`);
//! * `TagResource` is an UPSERT keyed by tag key: a key not held is added, a key
//!   already held has its value replaced, and no other tag is touched;
//! * `UntagResource` removes exactly the keys it names and is idempotent — a key
//!   the resource does not hold is not an error;
//! * an `AmazonResourceName` that names no stored resource is refused with
//!   `ResourceNotFoundException` (404) by every one of the three, and the
//!   refusal reports the ARN it could not resolve;
//! * a tag key or value outside the character set the model documents, or a key
//!   using the reserved `aws:` prefix, is refused with
//!   `InvalidRequestException` (400) and stores nothing;
//! * the published maximum of 50 user-applied tags per resource is reported with
//!   `TooManyTagsException` (400), never silently exceeded;
//! * the listing is `@paginated`: a `NextToken` only when it was truncated,
//!   resuming from it yields the remainder exactly once, and a token this
//!   service never issued is refused.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use service_com_amazonaws_xray_awsxray::ops::list_tags_for_resource::TAGS_PAGE_SIZE;
use service_com_amazonaws_xray_awsxray::state::MAX_TAGS_PER_RESOURCE;

use crate::xray_wire::{Answer, Xray};

/// The `TagList` JSON body for a set of `(key, value)` pairs.
fn tag_list<K: AsRef<str>, V: AsRef<str>>(tags: &[(K, V)]) -> Value {
    Value::Array(
        tags.iter()
            .map(|(key, value)| json!({"Key": key.as_ref(), "Value": value.as_ref()}))
            .collect(),
    )
}

/// Create a group through `CreateGroup` and answer the ARN it persisted.
///
/// Every group carries a filter expression, because X-Ray refuses a
/// `CreateGroup` that supplies none — see `group_control_plane.rs` for that
/// rule. Which expression is irrelevant here; these cases are about tags.
async fn create_group(xray: &Xray, name: &str, tags: &[(&str, &str)]) -> String {
    let mut body = json!({"GroupName": name, "FilterExpression": "ok"});
    if !tags.is_empty() {
        body["Tags"] = tag_list(tags);
    }
    let created = xray.call("/CreateGroup", &body).await;
    assert_eq!(
        created.status, 200,
        "creating group {name:?} must succeed, got {created:?}"
    );
    created
        .get("Group")
        .get("GroupARN")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("CreateGroup must answer a GroupARN, got {created:?}"))
        .to_string()
}

async fn apply_tags(xray: &Xray, arn: &str, tags: &Value) -> Answer {
    xray.call(
        "/TagResource",
        &json!({"ResourceARN": arn, "Tags": tags.clone()}),
    )
    .await
}

async fn remove_tags<K: AsRef<str>>(xray: &Xray, arn: &str, keys: &[K]) -> Answer {
    let keys: Vec<&str> = keys.iter().map(AsRef::as_ref).collect();
    xray.call(
        "/UntagResource",
        &json!({"ResourceARN": arn, "TagKeys": keys}),
    )
    .await
}

async fn list_page(xray: &Xray, arn: &str, token: Option<&str>) -> Answer {
    let mut body = json!({"ResourceARN": arn});
    if let Some(token) = token {
        body["NextToken"] = json!(token);
    }
    xray.call("/ListTagsForResource", &body).await
}

/// The tags one page carries, in the order they arrived. A tag missing either
/// half is a defect in the response, so it fails loudly rather than comparing
/// as an empty string.
fn page_tags(answer: &Answer) -> Vec<(String, String)> {
    answer
        .get("Tags")
        .as_array()
        .map(|tags| {
            tags.iter()
                .map(|tag| {
                    let member = |name: &str| {
                        tag.get(name)
                            .and_then(Value::as_str)
                            .unwrap_or_else(|| {
                                panic!("every listed tag must carry a string {name}, got {tag:?}")
                            })
                            .to_string()
                    };
                    (member("Key"), member("Value"))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Every tag of a resource, read through `ListTagsForResource` and following
/// `NextToken` to the end. Asserts on the way that no tag arrives twice.
async fn all_tags(xray: &Xray, arn: &str) -> BTreeMap<String, String> {
    let mut all = BTreeMap::new();
    let mut token: Option<String> = None;
    loop {
        let page = list_page(xray, arn, token.as_deref()).await;
        assert_eq!(
            page.status, 200,
            "listing the tags of {arn:?} must succeed, got {page:?}"
        );
        for (key, value) in page_tags(&page) {
            assert!(
                all.insert(key.clone(), value).is_none(),
                "tag {key:?} must be carried by exactly one page, got {page:?}"
            );
        }
        match page.get("NextToken").as_str() {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
    }
    all
}

/// The expected tag set, as a map.
fn expected_set<K: AsRef<str>, V: AsRef<str>>(tags: &[(K, V)]) -> BTreeMap<String, String> {
    tags.iter()
        .map(|(key, value)| (key.as_ref().to_string(), value.as_ref().to_string()))
        .collect()
}

fn pair(key: &str, value: &str) -> (String, String) {
    (key.to_string(), value.to_string())
}

// =============================================================================
// 1. A tag set is answered from the resource's stored record
// =============================================================================

/// One tag set. Rows differ in tag count, in the character classes the key and
/// the value carry, and in size at the published bounds.
struct TagSetCase {
    case: &'static str,
    group: &'static str,
    tags: Vec<(String, String)>,
}

fn tag_set_cases() -> Vec<TagSetCase> {
    vec![
        TagSetCase {
            case: "a single tag",
            group: "one",
            tags: vec![pair("Stage", "Production")],
        },
        TagSetCase {
            case: "several tags",
            group: "many",
            tags: vec![
                pair("Team", "payments"),
                pair("Owner", "sre"),
                pair("Env", "prod"),
            ],
        },
        TagSetCase {
            // `TagList`'s published @length min is 0.
            case: "no tags at all",
            group: "none",
            tags: vec![],
        },
        TagSetCase {
            // Every punctuation mark `TagResource$Tags` lists as valid for a key
            // and a value: _ . : / = + - and @
            case: "every punctuation character the documentation lists",
            group: "punctuation",
            tags: vec![pair("a_b.c:d/e=f+g-h@i", "j_k.l:m/n=o+p-q@r")],
        },
        TagSetCase {
            // ... plus the space the same sentence lists.
            case: "a space in both halves of the tag",
            group: "spaced",
            tags: vec![pair("Cost Center", "Shared Services")],
        },
        TagSetCase {
            // `TagValue`'s published @length min is 0.
            case: "the shortest value the model allows",
            group: "emptyvalue",
            tags: vec![pair("Backup", "")],
        },
        TagSetCase {
            // `TagKey`'s published @length min is 1.
            case: "the shortest key the model allows",
            group: "shortkey",
            tags: vec![pair("k", "v")],
        },
        TagSetCase {
            // `TagKey` max 128, `TagValue` max 256.
            case: "the longest key and value the model allows",
            group: "longest",
            tags: vec![pair(&"k".repeat(128), &"v".repeat(256))],
        },
        TagSetCase {
            // The `Tag` shape widens the alphanumeric half of the set to
            // "Unicode letters, numbers".
            case: "unicode letters and digits",
            group: "unicode",
            tags: vec![pair("Équipe", "Paiements2024"), pair("Ünïcode", "wërt")],
        },
    ]
}

#[tokio::test]
async fn tags_applied_to_a_resource_are_read_back_from_its_stored_record() {
    let xray = Xray::new();
    for case in tag_set_cases() {
        let label = case.case;
        let arn = create_group(&xray, case.group, &[]).await;
        let applied = apply_tags(&xray, &arn, &tag_list(&case.tags)).await;
        assert_eq!(
            (applied.status, applied.error_type.as_deref()),
            (200, None),
            "case {label:?}: TagResource must succeed, got {applied:?}"
        );
        let expected = expected_set(&case.tags);
        let got = all_tags(&xray, &arn).await;
        assert_eq!(
            got, expected,
            "case {label:?}: expected the stored tags {expected:?}, got {got:?}"
        );
    }
}

/// One creation-then-tagging case. Rows differ in whether the later call adds a
/// key, replaces a creation tag, or is not made at all.
struct CreationTagCase {
    case: &'static str,
    created_with: Vec<(&'static str, &'static str)>,
    applied_later: Vec<(&'static str, &'static str)>,
    expected: Vec<(&'static str, &'static str)>,
}

fn creation_tag_cases() -> Vec<CreationTagCase> {
    vec![
        CreationTagCase {
            case: "creation tags only",
            created_with: vec![("Stage", "Production"), ("Owner", "sre")],
            applied_later: vec![],
            expected: vec![("Stage", "Production"), ("Owner", "sre")],
        },
        CreationTagCase {
            case: "a later call adds a key",
            created_with: vec![("Stage", "Production")],
            applied_later: vec![("Team", "payments")],
            expected: vec![("Stage", "Production"), ("Team", "payments")],
        },
        CreationTagCase {
            case: "a later call replaces a creation tag",
            created_with: vec![("Stage", "Production"), ("Owner", "sre")],
            applied_later: vec![("Stage", "test-only")],
            expected: vec![("Stage", "test-only"), ("Owner", "sre")],
        },
    ]
}

#[tokio::test]
async fn tags_supplied_at_creation_are_listed_and_later_tags_merge_into_them() {
    let xray = Xray::new();
    for (index, case) in creation_tag_cases().iter().enumerate() {
        let label = case.case;
        let arn = create_group(&xray, &format!("created{index}"), &case.created_with).await;
        if !case.applied_later.is_empty() {
            let applied = apply_tags(&xray, &arn, &tag_list(&case.applied_later)).await;
            assert_eq!(
                applied.status, 200,
                "case {label:?}: TagResource must succeed, got {applied:?}"
            );
        }
        let expected = expected_set(&case.expected);
        let got = all_tags(&xray, &arn).await;
        assert_eq!(
            got, expected,
            "case {label:?}: expected the stored tags {expected:?}, got {got:?}"
        );
    }
}

// =============================================================================
// 2. TagResource is an upsert keyed by tag key
// =============================================================================

/// One upsert. Rows differ in the overlap between the stored keys and the keys
/// the second call names.
struct UpsertCase {
    case: &'static str,
    first: Vec<(&'static str, &'static str)>,
    second: Vec<(&'static str, &'static str)>,
    expected: Vec<(&'static str, &'static str)>,
}

fn upsert_cases() -> Vec<UpsertCase> {
    vec![
        UpsertCase {
            case: "disjoint keys are added",
            first: vec![("Alpha", "1"), ("Beta", "2")],
            second: vec![("Gamma", "3")],
            expected: vec![("Alpha", "1"), ("Beta", "2"), ("Gamma", "3")],
        },
        UpsertCase {
            case: "a key already held has its value replaced",
            first: vec![("Alpha", "1"), ("Beta", "2")],
            second: vec![("Alpha", "9")],
            expected: vec![("Alpha", "9"), ("Beta", "2")],
        },
        UpsertCase {
            case: "a partial overlap replaces only what it names",
            first: vec![("Alpha", "1"), ("Beta", "2"), ("Gamma", "3")],
            second: vec![("Beta", "20"), ("Delta", "4")],
            expected: vec![
                ("Alpha", "1"),
                ("Beta", "20"),
                ("Gamma", "3"),
                ("Delta", "4"),
            ],
        },
        UpsertCase {
            case: "a key repeated inside one request ends at its last value",
            first: vec![],
            second: vec![("Alpha", "1"), ("Alpha", "2")],
            expected: vec![("Alpha", "2")],
        },
        UpsertCase {
            case: "re-applying the same value changes nothing",
            first: vec![("Alpha", "1")],
            second: vec![("Alpha", "1")],
            expected: vec![("Alpha", "1")],
        },
    ]
}

#[tokio::test]
async fn tagging_replaces_the_keys_it_names_and_leaves_every_other_tag_alone() {
    let xray = Xray::new();
    for (index, case) in upsert_cases().iter().enumerate() {
        let label = case.case;
        let arn = create_group(&xray, &format!("upsert{index}"), &[]).await;
        if !case.first.is_empty() {
            let seeded = apply_tags(&xray, &arn, &tag_list(&case.first)).await;
            assert_eq!(
                seeded.status, 200,
                "case {label:?}: seeding must succeed, got {seeded:?}"
            );
        }
        let applied = apply_tags(&xray, &arn, &tag_list(&case.second)).await;
        assert_eq!(
            applied.status, 200,
            "case {label:?}: TagResource must succeed, got {applied:?}"
        );
        let expected = expected_set(&case.expected);
        let got = all_tags(&xray, &arn).await;
        assert_eq!(
            got, expected,
            "case {label:?}: expected the stored tags {expected:?}, got {got:?}"
        );
    }
}

// =============================================================================
// 3. UntagResource removes the keys it names, and only those
// =============================================================================

/// One removal. Rows differ in how the named keys overlap the stored ones, and
/// in how many are named.
struct UntagCase {
    case: &'static str,
    initial: Vec<(&'static str, &'static str)>,
    remove: Vec<&'static str>,
    expected: Vec<(&'static str, &'static str)>,
}

fn untag_cases() -> Vec<UntagCase> {
    vec![
        UntagCase {
            case: "one named key",
            initial: vec![("Alpha", "1"), ("Beta", "2"), ("Gamma", "3")],
            remove: vec!["Beta"],
            expected: vec![("Alpha", "1"), ("Gamma", "3")],
        },
        UntagCase {
            case: "a key the resource does not hold removes nothing",
            initial: vec![("Alpha", "1")],
            remove: vec!["Zeta"],
            expected: vec![("Alpha", "1")],
        },
        UntagCase {
            case: "a mix of held and unheld keys",
            initial: vec![("Alpha", "1"), ("Beta", "2")],
            remove: vec!["Alpha", "Zeta"],
            expected: vec![("Beta", "2")],
        },
        UntagCase {
            // `TagKeyList`'s published @length min is 0.
            case: "an empty key list removes nothing",
            initial: vec![("Alpha", "1"), ("Beta", "2")],
            remove: vec![],
            expected: vec![("Alpha", "1"), ("Beta", "2")],
        },
        UntagCase {
            case: "every key the resource holds",
            initial: vec![("Alpha", "1"), ("Beta", "2")],
            remove: vec!["Alpha", "Beta"],
            expected: vec![],
        },
        UntagCase {
            case: "the same key named twice in one request",
            initial: vec![("Alpha", "1"), ("Beta", "2")],
            remove: vec!["Alpha", "Alpha"],
            expected: vec![("Beta", "2")],
        },
    ]
}

#[tokio::test]
async fn untagging_removes_the_named_keys_only_and_is_idempotent() {
    let xray = Xray::new();
    for (index, case) in untag_cases().iter().enumerate() {
        let label = case.case;
        let arn = create_group(&xray, &format!("untag{index}"), &[]).await;
        if !case.initial.is_empty() {
            let seeded = apply_tags(&xray, &arn, &tag_list(&case.initial)).await;
            assert_eq!(
                seeded.status, 200,
                "case {label:?}: seeding must succeed, got {seeded:?}"
            );
        }
        let expected = expected_set(&case.expected);
        // Twice: a removal already made is still a success and still leaves the
        // same set, because the operation is idempotent.
        for attempt in 1..=2 {
            let removed = remove_tags(&xray, &arn, &case.remove).await;
            assert_eq!(
                (removed.status, removed.error_type.as_deref()),
                (200, None),
                "case {label:?}, attempt {attempt}: UntagResource must succeed, got {removed:?}"
            );
            let got = all_tags(&xray, &arn).await;
            assert_eq!(
                got, expected,
                "case {label:?}, attempt {attempt}: expected the stored tags {expected:?}, got {got:?}"
            );
        }
    }
}

// =============================================================================
// 4. An ARN that names no stored resource
// =============================================================================

#[tokio::test]
async fn a_tagging_call_on_an_arn_naming_no_resource_is_refused_with_not_found() {
    let xray = Xray::new();
    let live = create_group(&xray, "live", &[("Keep", "yes")]).await;
    let deleted = create_group(&xray, "doomed", &[]).await;
    let gone = xray
        .call("/DeleteGroup", &json!({"GroupARN": &deleted}))
        .await;
    assert_eq!(
        gone.status, 200,
        "deleting the group must succeed, got {gone:?}"
    );

    // The live group's ARN with another unique suffix: well formed, carried by
    // no stored group.
    let mut unknown_suffix = live.clone();
    unknown_suffix.truncate(live.len() - 12);
    unknown_suffix.push_str("ffffffffffff");

    // Rows differ in the FORM of the unresolvable ARN.
    let cases: Vec<(&str, String)> = vec![
        ("a well-formed group ARN no group carries", unknown_suffix),
        ("the ARN of a deleted group", deleted.clone()),
        (
            "a group ARN whose group name was never created",
            "arn:aws:xray:us-east-1:000000000000:group/absent/000000000000".to_string(),
        ),
        (
            "a sampling-rule ARN, which this emulator stores none of",
            "arn:aws:xray:us-east-1:000000000000:sampling-rule/absent".to_string(),
        ),
        ("another service's ARN", "arn:aws:s3:::some-bucket".to_string()),
        // `AmazonResourceName`'s published @length min is 1.
        ("the shortest ARN the model allows", "x".to_string()),
    ];

    for (label, arn) in &cases {
        let calls = [
            (
                "/TagResource",
                json!({"ResourceARN": arn, "Tags": [{"Key": "Stage", "Value": "Production"}]}),
            ),
            (
                "/UntagResource",
                json!({"ResourceARN": arn, "TagKeys": ["Stage"]}),
            ),
            ("/ListTagsForResource", json!({"ResourceARN": arn})),
        ];
        for (path, body) in &calls {
            let refused = xray.call(path, body).await;
            assert_eq!(
                (refused.status, refused.error_type.as_deref()),
                (404, Some("ResourceNotFoundException")),
                "case {label:?}: {path} on an ARN naming no resource must be refused, got {refused:?}"
            );
            assert_eq!(
                refused.get("ResourceName").as_str(),
                Some(arn.as_str()),
                "case {label:?}: {path} must report the ARN it could not resolve, got {refused:?}"
            );
        }
    }

    // None of those refusals touched the resource that does exist.
    let got = all_tags(&xray, &live).await;
    assert_eq!(
        got,
        expected_set(&[("Keep", "yes")]),
        "the live group's tags must be untouched by refused calls, got {got:?}"
    );
}

// =============================================================================
// 5. A tag a caller may not apply
// =============================================================================

/// One refused tag. Rows differ in which half of the tag is at fault and in
/// which restriction it breaks.
struct RejectedTagCase {
    case: &'static str,
    key: &'static str,
    value: &'static str,
}

fn rejected_tag_cases() -> Vec<RejectedTagCase> {
    vec![
        RejectedTagCase {
            case: "a key using the reserved aws: prefix",
            key: "aws:cloudformation:stack-name",
            value: "a-stack",
        },
        RejectedTagCase {
            case: "a key carrying a character outside the documented set",
            key: "bad#key",
            value: "fine",
        },
        RejectedTagCase {
            case: "a value carrying a character outside the documented set",
            key: "Fine",
            value: "bad#value",
        },
        RejectedTagCase {
            case: "a key carrying a comma",
            key: "a,b",
            value: "fine",
        },
        RejectedTagCase {
            case: "a value carrying a tab",
            key: "Fine",
            value: "a\tb",
        },
        RejectedTagCase {
            case: "a key carrying a brace",
            key: "{key}",
            value: "fine",
        },
    ]
}

#[tokio::test]
async fn a_tag_the_documentation_forbids_is_refused_and_stores_nothing() {
    let xray = Xray::new();
    let seeded = [("Stage", "Production")];
    for (index, case) in rejected_tag_cases().iter().enumerate() {
        let label = case.case;
        let arn = create_group(&xray, &format!("rejected{index}"), &seeded).await;
        // The refused tag travels beside a perfectly good one, so the assertion
        // below also shows that NOTHING from a refused call was stored.
        let tags = tag_list(&[(case.key, case.value), ("Also", "valid")]);
        let refused = apply_tags(&xray, &arn, &tags).await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("InvalidRequestException")),
            "case {label:?}: TagResource must refuse the tag, got {refused:?}"
        );
        let got = all_tags(&xray, &arn).await;
        assert_eq!(
            got,
            expected_set(&seeded),
            "case {label:?}: a refused call must store nothing, got {got:?}"
        );
    }
}

#[tokio::test]
async fn a_tag_key_a_caller_may_not_name_is_refused_by_untag_resource_too() {
    let xray = Xray::new();
    let seeded = [("Stage", "Production")];
    // Rows differ in which restriction the named key breaks.
    let cases = [
        ("the reserved aws: prefix, which marks a system tag", "aws:created-by"),
        ("a character outside the documented set", "bad#key"),
        ("a comma", "a,b"),
    ];
    for (index, (label, key)) in cases.iter().enumerate() {
        let arn = create_group(&xray, &format!("untagbad{index}"), &seeded).await;
        let refused = remove_tags(&xray, &arn, &[*key, "Stage"]).await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("InvalidRequestException")),
            "case {label:?}: UntagResource must refuse key {key:?}, got {refused:?}"
        );
        let got = all_tags(&xray, &arn).await;
        assert_eq!(
            got,
            expected_set(&seeded),
            "case {label:?}: a refused call must remove nothing, got {got:?}"
        );
    }
}

// =============================================================================
// 6. The published per-resource tag cap
// =============================================================================

#[tokio::test]
async fn the_published_per_resource_tag_cap_is_reported_never_exceeded() {
    let xray = Xray::new();
    let arn = create_group(&xray, "capped", &[]).await;

    // Exactly at the cap: accepted.
    let full: Vec<(String, String)> = (0..MAX_TAGS_PER_RESOURCE)
        .map(|n| (format!("key{n:03}"), format!("value{n:03}")))
        .collect();
    let applied = apply_tags(&xray, &arn, &tag_list(&full)).await;
    assert_eq!(
        applied.status, 200,
        "{MAX_TAGS_PER_RESOURCE} tags are the published maximum and must be accepted, got {applied:?}"
    );
    let got = all_tags(&xray, &arn).await;
    assert_eq!(
        got,
        expected_set(&full),
        "every tag at the cap must be stored, got {} of them",
        got.len()
    );

    // At the cap, replacing a key already held does not grow the set.
    let replaced = apply_tags(&xray, &arn, &tag_list(&[("key000", "changed")])).await;
    assert_eq!(
        replaced.status, 200,
        "replacing a held key at the cap must be accepted, got {replaced:?}"
    );
    let got = all_tags(&xray, &arn).await;
    assert_eq!(
        got.get("key000").map(String::as_str),
        Some("changed"),
        "the replacement must be stored, got {:?}",
        got.get("key000")
    );
    assert_eq!(
        got.len(),
        MAX_TAGS_PER_RESOURCE,
        "replacing a key must leave the tag count at the cap, got {}",
        got.len()
    );

    // One key past the cap: refused, and nothing stored.
    let overflow = apply_tags(&xray, &arn, &tag_list(&[("overflow", "x")])).await;
    assert_eq!(
        (overflow.status, overflow.error_type.as_deref()),
        (400, Some("TooManyTagsException")),
        "one tag past the cap must be refused, got {overflow:?}"
    );
    assert_eq!(
        overflow.get("ResourceName").as_str(),
        Some(arn.as_str()),
        "the refusal must name the resource it was about, got {overflow:?}"
    );
    let got = all_tags(&xray, &arn).await;
    assert!(
        got.len() == MAX_TAGS_PER_RESOURCE && !got.contains_key("overflow"),
        "a refused call must store nothing, got {} tags and overflow={:?}",
        got.len(),
        got.get("overflow")
    );

    // And one call carrying one key past the cap is refused the same way.
    let fresh = create_group(&xray, "capped-at-once", &[]).await;
    let too_many: Vec<(String, String)> = (0..=MAX_TAGS_PER_RESOURCE)
        .map(|n| (format!("key{n:03}"), format!("value{n:03}")))
        .collect();
    let refused = apply_tags(&xray, &fresh, &tag_list(&too_many)).await;
    assert_eq!(
        (refused.status, refused.error_type.as_deref()),
        (400, Some("TooManyTagsException")),
        "{} tags in one call must be refused, got {refused:?}",
        too_many.len()
    );
    let got = all_tags(&xray, &fresh).await;
    assert!(
        got.is_empty(),
        "a refused call must store nothing, got {got:?}"
    );
}

// =============================================================================
// 7. The listing's @paginated contract
// =============================================================================

#[tokio::test]
async fn the_tag_listing_pages_and_resuming_yields_the_remainder_exactly_once() {
    let xray = Xray::new();
    // Rows differ in the tag count relative to one page: below it, exactly it,
    // one past it, and several past it.
    for count in [
        1usize,
        TAGS_PAGE_SIZE,
        TAGS_PAGE_SIZE + 1,
        TAGS_PAGE_SIZE + 5,
    ] {
        let arn = create_group(&xray, &format!("paged{count}"), &[]).await;
        let tags: Vec<(String, String)> = (0..count)
            .map(|n| (format!("key{n:03}"), format!("value{n:03}")))
            .collect();
        let applied = apply_tags(&xray, &arn, &tag_list(&tags)).await;
        assert_eq!(
            applied.status, 200,
            "case {count} tags: TagResource must succeed, got {applied:?}"
        );

        let first = list_page(&xray, &arn, None).await;
        assert_eq!(
            first.status, 200,
            "case {count} tags: the first page must succeed, got {first:?}"
        );
        let page = page_tags(&first);
        let expected_len = count.min(TAGS_PAGE_SIZE);
        assert_eq!(
            page.len(),
            expected_len,
            "case {count} tags: expected {expected_len} on the first page, got {}",
            page.len()
        );
        let mut keys: Vec<&str> = page.iter().map(|(key, _)| key.as_str()).collect();
        let listed = keys.clone();
        keys.sort_unstable();
        assert_eq!(
            listed, keys,
            "case {count} tags: a page must be in tag-key order, got {listed:?}"
        );

        let token = first.get("NextToken").as_str().map(str::to_string);
        if count > TAGS_PAGE_SIZE {
            assert_eq!(
                token.as_deref(),
                Some(tags[TAGS_PAGE_SIZE].0.as_str()),
                "case {count} tags: a truncated listing must resume at the next key, got {token:?}"
            );
        } else {
            assert!(
                token.is_none(),
                "case {count} tags: an untruncated listing must carry no NextToken, got {token:?}"
            );
        }

        let got = all_tags(&xray, &arn).await;
        assert_eq!(
            got,
            expected_set(&tags),
            "case {count} tags: paging through must yield every tag exactly once, got {} of them",
            got.len()
        );
    }

    // A token this service never issued is refused, not answered with a page.
    let arn = create_group(&xray, "tokens", &[("Stage", "Production")]).await;
    let refused = list_page(&xray, &arn, Some("~never-issued~")).await;
    assert_eq!(
        (refused.status, refused.error_type.as_deref()),
        (400, Some("InvalidRequestException")),
        "a NextToken this service never issued must be refused, got {refused:?}"
    );
}
