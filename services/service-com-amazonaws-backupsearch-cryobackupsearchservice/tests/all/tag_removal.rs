// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for `UntagResource` (`DELETE /tags/{ResourceArn}?tagKeys=…`), driven
//! over HTTP through the same harness the search-job suite uses.
//!
//! The rule under test: `UntagResource` deletes from the tag set stored on the resource
//! the ARN names exactly the keys `TagKeys` lists and leaves every other key as it was,
//! so `ListTagsForResource` afterwards reports exactly the surviving set; because the
//! operation is `@idempotent`, a key the resource does not carry removes nothing and the
//! call still succeeds. So every assertion is made over a TABLE of removals that differ
//! in the members the rule generalises over — how many keys are named, whether they are
//! present, their order, whether one is repeated, their letter case and the punctuation
//! in them — never over the one request the issue happened to cite.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The crate's ONE HTTP test harness lives in the search-job suite; both suites drive the
// same emulator the same way, so it is shared rather than written twice.
use crate::search_jobs::{call, emulator, percent_encode, Answer};

/// The region every call in this suite is signed for. The rule does not vary by region
/// (the search-job suite already walks the regions), so one is enough here.
const REGION: &str = "us-west-2";

/// One `UntagResource` call and the tag set the resource must report once it has been
/// made. The rows are applied IN ORDER against one job, so each row's `remaining` is the
/// full state of that job's tags after the removal.
struct RemovalCase {
    case: &'static str,
    /// The `TagKeys` the call names.
    keys: &'static [&'static str],
    /// Every key/value pair the resource must still carry afterwards.
    remaining: &'static [(&'static str, &'static str)],
}

/// The tag set each job in this suite is created with.
///
/// The keys exercise the character classes `Tags` documents for this service ("UTF-8
/// letters, numbers, spaces, and the following characters: + - = . _ : /") beside plain
/// ones, so a removal is tested on a key the issue did not cite.
fn seed_tags() -> Value {
    json!({
        "env": "test",
        "team": "a",
        "tier": "gold",
        "owner": "platform",
        "cost:center": "cc-1",
        "app/name+1": "search",
        "two words": "yes"
    })
}

/// `DELETE /tags/{ResourceArn}?tagKeys=…`: `TagKeys` is an `@httpQuery` list, so each key
/// is its own repeated `tagKeys=` parameter.
fn untag_uri(arn: &str, keys: &[&str]) -> String {
    let query: Vec<String> = keys
        .iter()
        .map(|key| format!("tagKeys={}", query_value(key)))
        .collect();
    format!("/tags/{}?{}", percent_encode(arn), query.join("&"))
}

/// A query-string value with every byte outside RFC 3986's unreserved set
/// percent-encoded. `+` in particular HAS to be escaped: a query value is decoded with
/// `+` meaning a space, so an unescaped `+` would name a different key than the one
/// stored. The sibling suite's `percent_encode` is the path-label encoder (it escapes
/// only `/`), which cannot serve a query value.
///
/// Written out here rather than pulled from `percent-encoding`: a dependency edge is a
/// `Cargo.lock` edit, and every concurrently landing fix rewrites that file, which made
/// this change unlandable (the gate's squash merge refused it with
/// ManualMergeRequiredException on three successive submits that all ten reviewers had
/// approved). Eight lines, test-only, over values this file writes.
fn query_value(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                char::from(byte).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// The tags `ListTagsForResource` reports for `arn`, as a sorted map so a comparison does
/// not depend on map ordering.
async fn stored_tags(emulator: &MultiServiceRouter, arn: &str) -> BTreeMap<String, String> {
    let listed = call(
        emulator,
        REGION,
        "GET",
        &format!("/tags/{}", percent_encode(arn)),
        None,
    )
    .await;
    assert_eq!(
        listed.status, 200,
        "reading the tags of {arn:?} must be 200, got {} ({:?})",
        listed.status, listed.body
    );
    as_map(listed.get("Tags"))
}

/// A JSON object of string values as a sorted map.
fn as_map(value: &Value) -> BTreeMap<String, String> {
    value
        .as_object()
        .map(|members| {
            members
                .iter()
                .map(|(key, value)| (key.clone(), value.as_str().unwrap_or_default().to_string()))
                .collect()
        })
        .unwrap_or_default()
}

/// The expected set of a row, as a sorted map.
fn wanted(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// Start one search job carrying [`seed_tags`] and answer its ARN.
async fn tagged_job(emulator: &MultiServiceRouter, name: &str) -> String {
    let started = call(
        emulator,
        REGION,
        "PUT",
        "/search-jobs",
        Some(json!({
            "Name": name,
            "SearchScope": {"BackupResourceTypes": ["S3"]},
            "Tags": seed_tags()
        })),
    )
    .await;
    assert_eq!(
        started.status, 200,
        "case {name:?}: StartSearchJob must succeed, got {} ({:?})",
        started.status, started.body
    );
    started.string("SearchJobArn")
}

fn removal_cases() -> Vec<RemovalCase> {
    vec![
        RemovalCase {
            case: "one key the resource carries",
            keys: &["env"],
            remaining: &[
                ("team", "a"),
                ("tier", "gold"),
                ("owner", "platform"),
                ("cost:center", "cc-1"),
                ("app/name+1", "search"),
                ("two words", "yes"),
            ],
        },
        RemovalCase {
            case: "the same key again, after it is already gone",
            keys: &["env"],
            remaining: &[
                ("team", "a"),
                ("tier", "gold"),
                ("owner", "platform"),
                ("cost:center", "cc-1"),
                ("app/name+1", "search"),
                ("two words", "yes"),
            ],
        },
        RemovalCase {
            case: "a key no resource of this account ever carried",
            keys: &["never-set"],
            remaining: &[
                ("team", "a"),
                ("tier", "gold"),
                ("owner", "platform"),
                ("cost:center", "cc-1"),
                ("app/name+1", "search"),
                ("two words", "yes"),
            ],
        },
        RemovalCase {
            case: "a key differing from a stored one only in case",
            keys: &["TEAM"],
            remaining: &[
                ("team", "a"),
                ("tier", "gold"),
                ("owner", "platform"),
                ("cost:center", "cc-1"),
                ("app/name+1", "search"),
                ("two words", "yes"),
            ],
        },
        RemovalCase {
            case: "two keys in one call",
            keys: &["team", "tier"],
            remaining: &[
                ("owner", "platform"),
                ("cost:center", "cc-1"),
                ("app/name+1", "search"),
                ("two words", "yes"),
            ],
        },
        RemovalCase {
            case: "a present key beside an absent one",
            keys: &["never-set", "owner"],
            remaining: &[
                ("cost:center", "cc-1"),
                ("app/name+1", "search"),
                ("two words", "yes"),
            ],
        },
        RemovalCase {
            case: "a key carrying the punctuation tags allow",
            keys: &["cost:center"],
            remaining: &[("app/name+1", "search"), ("two words", "yes")],
        },
        RemovalCase {
            case: "the same key named twice in one call",
            keys: &["app/name+1", "app/name+1"],
            remaining: &[("two words", "yes")],
        },
        RemovalCase {
            case: "the last key the resource carries",
            keys: &["two words"],
            remaining: &[],
        },
        RemovalCase {
            case: "a key on a resource that now carries none",
            keys: &["two words"],
            remaining: &[],
        },
    ]
}

#[tokio::test]
async fn untag_removes_exactly_the_named_keys_and_leaves_the_rest() {
    let emulator = emulator();
    let arn = tagged_job(&emulator, "untag-table").await;

    for RemovalCase {
        case,
        keys,
        remaining,
    } in removal_cases()
    {
        let removed = call(&emulator, REGION, "DELETE", &untag_uri(&arn, keys), None).await;
        assert_eq!(
            removed.status, 200,
            "case {case:?}: UntagResource must answer 200, got {} ({:?})",
            removed.status, removed.body
        );
        assert!(
            removed.error_code.is_none(),
            "case {case:?}: a successful UntagResource must carry no error code, got {:?}",
            removed.error_code
        );

        let actual = stored_tags(&emulator, &arn).await;
        assert_eq!(
            actual,
            wanted(remaining),
            "case {case:?}: the resource must report exactly the surviving tags, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn untag_is_idempotent_so_repeating_a_call_reports_the_same_set() {
    let emulator = emulator();
    let arn = tagged_job(&emulator, "untag-idempotent").await;

    // Rows differ in how many keys the call names and how many of them are present, so
    // idempotence is pinned across the whole class and not only on a single-key removal.
    let calls: Vec<(&str, &[&str])> = vec![
        ("one present key", &["env"]),
        ("two present keys", &["team", "tier"]),
        ("one absent key", &["never-set"]),
        ("a present and an absent key", &["owner", "never-set"]),
    ];

    for (case, keys) in calls {
        let uri = untag_uri(&arn, keys);
        let first = call(&emulator, REGION, "DELETE", &uri, None).await;
        assert_eq!(
            first.status, 200,
            "case {case:?}: the first UntagResource must be 200, got {} ({:?})",
            first.status, first.body
        );
        let after_first = stored_tags(&emulator, &arn).await;

        let again = call(&emulator, REGION, "DELETE", &uri, None).await;
        assert_eq!(
            again.status, 200,
            "case {case:?}: repeating UntagResource must be 200, got {} ({:?})",
            again.status, again.body
        );
        let after_second = stored_tags(&emulator, &arn).await;
        assert_eq!(
            after_second, after_first,
            "case {case:?}: repeating the same removal must not change the stored set, got {after_second:?}"
        );
        for key in keys {
            assert!(
                !after_second.contains_key(*key),
                "case {case:?}: key {key:?} must be gone, got {after_second:?}"
            );
        }
    }
}

#[tokio::test]
async fn untag_touches_only_the_resource_its_arn_names() {
    let emulator = emulator();
    // Two jobs holding the same keys: a removal addressed at one must leave the other's
    // set whole, so the handler resolves the record by ARN rather than by key.
    let first = tagged_job(&emulator, "untag-first").await;
    let second = tagged_job(&emulator, "untag-second").await;
    assert_ne!(
        first, second,
        "two started jobs must have different ARNs, got {first:?} twice"
    );

    let untagged = call(
        &emulator,
        REGION,
        "DELETE",
        &untag_uri(&first, &["env", "team"]),
        None,
    )
    .await;
    assert_eq!(
        untagged.status, 200,
        "UntagResource must answer 200, got {} ({:?})",
        untagged.status, untagged.body
    );

    let left = stored_tags(&emulator, &first).await;
    for key in ["env", "team"] {
        assert!(
            !left.contains_key(key),
            "case {key:?}: the addressed resource must have lost the key, got {left:?}"
        );
    }
    let untouched = stored_tags(&emulator, &second).await;
    assert_eq!(
        untouched,
        as_map(&seed_tags()),
        "the other resource must keep every tag it was created with, got {untouched:?}"
    );
}

#[tokio::test]
async fn untagging_an_arn_no_resource_matches_is_not_found_and_writes_nothing() {
    let emulator = emulator();
    let existing = tagged_job(&emulator, "untag-not-found").await;

    // Rows differ in HOW the ARN fails to resolve: an identifier no job holds, an ARN of
    // another service's resource type, and a string that is not an ARN at all.
    let unknown: Vec<(&str, String)> = vec![
        (
            "an identifier no job holds",
            "arn:aws:backup-search:us-west-2:000000000000:search-job/does-not-exist".to_string(),
        ),
        (
            "an ARN naming another service's resource",
            "arn:aws:s3:::some-bucket".to_string(),
        ),
        ("a string that is not an ARN", "not-an-arn".to_string()),
    ];

    for (case, arn) in unknown {
        let answer: Answer =
            call(&emulator, REGION, "DELETE", &untag_uri(&arn, &["env"]), None).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: an unknown resource must be 404, got {} ({:?})",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case:?}: the refusal must carry ResourceNotFoundException, got {:?}",
            answer.error_code
        );
    }

    // And the refusals left the resource that DOES exist exactly as it was.
    let untouched = stored_tags(&emulator, &existing).await;
    assert_eq!(
        untouched,
        as_map(&seed_tags()),
        "a refused UntagResource must not have removed anything, got {untouched:?}"
    );
}
