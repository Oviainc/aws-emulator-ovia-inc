// SPDX-License-Identifier: CC0-1.0

//! `ListNamedShadowsForThing` when there is nothing to list, end to end over HTTP
//! through the router this service mounts in production.
//!
//! The rule these cases assert: this operation is a LISTING, so "no items" is an empty
//! collection and not a missing resource. Whatever the thing's shadow store holds — a
//! thing never written to, a thing holding only the unnamed (classic) shadow, or a thing
//! whose named shadows have all been deleted — the answer is HTTP 200 carrying `results`
//! (the named shadows that currently hold a document, in stable key order, never the
//! classic one) and the required `timestamp`, with no `nextToken` once the page reaches
//! the end. `ResourceNotFoundException` stays reserved for a request that addresses a
//! shadow, which a listing never does.
//!
//! The table spans what the rule generalises over: how many named shadows were ever
//! written, how many remain, whether a classic shadow is present, the form and length of
//! the thing and shadow names, and the page size and token the listing is asked for. The
//! recorded cases (a never-written thing, and a thing whose named shadows were deleted)
//! are two rows among them.

use serde_json::{json, Value};

use crate::shadow_and_retained_message::{Answer, Harness};

/// The published bounds of the `pageSize` range, as
/// `smithy_query(query="operation", operation="ListNamedShadowsForThing")` states it
/// (`range {min: 1, max: 100}`).
const PAGE_SIZE_FLOOR: i64 = 1;
const PAGE_SIZE_CEILING: i64 = 100;

/// The maximum length of a `shadowName`, as the model's `ShadowName` states it
/// (`length {min: 1, max: 64}`).
const SHADOW_NAME_MAX: usize = 64;

/// The maximum length of a `thingName`, as the model's `ThingName` states it
/// (`length {min: 1, max: 128}`).
const THING_NAME_MAX: usize = 128;

/// The shadow-store setup of one row, and the listing it must then answer.
struct ListingCase {
    name: &'static str,
    thing: String,
    /// Shadows to write, in order. The empty string is the unnamed (classic) shadow.
    write: Vec<String>,
    /// Shadows to delete afterwards, in order. The empty string is the classic shadow.
    delete: Vec<String>,
    /// The `results` the listing must then carry, in the order it must carry them.
    expected: Vec<String>,
}

fn names(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_string()).collect()
}

/// Rows differing in shadows written, shadows deleted, presence of a classic shadow, and
/// the form and length of the thing and shadow names.
fn listing_cases() -> Vec<ListingCase> {
    vec![
        ListingCase {
            name: "a thing never written to at all",
            thing: "unseen-thing".to_string(),
            write: Vec::new(),
            delete: Vec::new(),
            expected: Vec::new(),
        },
        ListingCase {
            name: "a single-character thing name never written to",
            thing: "a".to_string(),
            write: Vec::new(),
            delete: Vec::new(),
            expected: Vec::new(),
        },
        ListingCase {
            name: "a thing name at its maximum length, never written to",
            thing: "t".repeat(THING_NAME_MAX),
            write: Vec::new(),
            delete: Vec::new(),
            expected: Vec::new(),
        },
        ListingCase {
            name: "a thing name of colons, underscores and dashes, never written to",
            thing: "fleet:1_edge-9".to_string(),
            write: Vec::new(),
            delete: Vec::new(),
            expected: Vec::new(),
        },
        ListingCase {
            name: "a thing holding only the classic shadow",
            thing: "classic-only-thing".to_string(),
            write: names(&[""]),
            delete: Vec::new(),
            expected: Vec::new(),
        },
        ListingCase {
            name: "a thing whose only named shadow was deleted",
            thing: "one-deleted-thing".to_string(),
            write: names(&["cfg"]),
            delete: names(&["cfg"]),
            expected: Vec::new(),
        },
        ListingCase {
            name: "a thing whose three named shadows were all deleted",
            thing: "all-deleted-thing".to_string(),
            write: names(&["alpha", "beta", "gamma"]),
            delete: names(&["gamma", "alpha", "beta"]),
            expected: Vec::new(),
        },
        ListingCase {
            name: "a thing keeping its classic shadow with every named shadow deleted",
            thing: "classic-survives-thing".to_string(),
            write: names(&["", "telemetry-1"]),
            delete: names(&["telemetry-1"]),
            expected: Vec::new(),
        },
        ListingCase {
            name: "a thing whose named and classic shadows were both deleted",
            thing: "everything-deleted-thing".to_string(),
            write: names(&["", "sensor"]),
            delete: names(&["sensor", ""]),
            expected: Vec::new(),
        },
        ListingCase {
            name: "a thing holding one named shadow",
            thing: "one-named-thing".to_string(),
            write: names(&["only"]),
            delete: Vec::new(),
            expected: names(&["only"]),
        },
        ListingCase {
            name: "a thing holding one of three named shadows",
            thing: "partly-deleted-thing".to_string(),
            write: names(&["alpha", "beta", "gamma"]),
            delete: names(&["alpha", "gamma"]),
            expected: names(&["beta"]),
        },
        ListingCase {
            name: "a thing holding named shadows beside a classic one",
            thing: "mixed-thing".to_string(),
            write: names(&["", "zz-last", "aa-first"]),
            delete: Vec::new(),
            expected: names(&["aa-first", "zz-last"]),
        },
        ListingCase {
            name: "a shadow name at its maximum length, written then deleted",
            thing: "long-name-thing".to_string(),
            write: vec!["s".repeat(SHADOW_NAME_MAX)],
            delete: vec!["s".repeat(SHADOW_NAME_MAX)],
            expected: Vec::new(),
        },
    ]
}

/// The shadow argument a name stands for: the empty string is the classic shadow.
fn shadow_of(name: &str) -> Option<&str> {
    (!name.is_empty()).then_some(name)
}

fn state_document() -> Vec<u8> {
    serde_json::to_vec(&json!({"state": {"reported": {"edge": "ok"}}})).expect("serializable")
}

/// The `results` of a listing that must have succeeded.
fn results(case: &str, answer: &Answer) -> Vec<String> {
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: expected 200, got {} with body {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    let body = answer.json();
    let results = body.get("results").and_then(Value::as_array).unwrap_or_else(|| {
        panic!("case {case:?}: expected a results list on the listing, got {body}")
    });
    results
        .iter()
        .map(|value| {
            value
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected a shadow name, got {value}"))
                .to_string()
        })
        .collect()
}

/// The `timestamp` of a listing, which the model marks required on the 200 response and
/// which this data plane reports in epoch SECONDS.
fn assert_timestamp(case: &str, answer: &Answer) {
    let body = answer.json();
    let timestamp = body
        .get("timestamp")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("case {case:?}: expected a timestamp on the listing, got {body}"));
    // 1_600_000_000 s is 2020-09-13, and the same instant in MILLISECONDS would be a
    // thousand times larger, so this bounds the unit as well as the value.
    assert!(
        (1_600_000_000..100_000_000_000).contains(&timestamp),
        "case {case:?}: expected a timestamp in epoch seconds, got {timestamp}"
    );
}

fn next_token(answer: &Answer) -> Option<String> {
    answer
        .json()
        .get("nextToken")
        .and_then(Value::as_str)
        .map(str::to_string)
}

#[tokio::test]
async fn a_listing_with_no_items_is_an_empty_results_list_and_never_a_not_found() {
    let harness = Harness::new();

    for case in listing_cases() {
        for shadow in &case.write {
            let written = harness
                .update(&case.thing, shadow_of(shadow), state_document())
                .await;
            assert_eq!(
                written.status,
                200,
                "case {:?}: expected the setup update of {shadow:?} to be accepted, got {} with \
                 body {}",
                case.name,
                written.status,
                String::from_utf8_lossy(&written.body)
            );
        }
        for shadow in &case.delete {
            let removed = harness.delete_shadow(&case.thing, shadow_of(shadow)).await;
            assert_eq!(
                removed.status,
                200,
                "case {:?}: expected the setup delete of {shadow:?} to be accepted, got {} with \
                 body {}",
                case.name,
                removed.status,
                String::from_utf8_lossy(&removed.body)
            );
        }

        let listed = harness.list_named_shadows(&case.thing, "").await;
        assert_eq!(
            results(case.name, &listed),
            case.expected,
            "case {:?}: expected the named shadows holding a document, got body {}",
            case.name,
            String::from_utf8_lossy(&listed.body)
        );
        assert_timestamp(case.name, &listed);
        assert_eq!(
            next_token(&listed),
            None,
            "case {:?}: expected no continuation when one page holds every name, got body {}",
            case.name,
            String::from_utf8_lossy(&listed.body)
        );
    }
}

#[tokio::test]
async fn an_empty_listing_pages_and_continues_like_any_other() {
    let harness = Harness::new();

    // The same empty listing, asked for at both published bounds of `pageSize`, at a
    // size in between, with no `pageSize` at all, and from a token this listing could
    // only have issued when it was not empty.
    for (case, query) in [
        ("no page size", String::new()),
        ("page size at the floor", format!("?pageSize={PAGE_SIZE_FLOOR}")),
        ("page size in the middle", "?pageSize=7".to_string()),
        ("page size at the ceiling", format!("?pageSize={PAGE_SIZE_CEILING}")),
        ("the first offset", "?nextToken=0".to_string()),
        ("an offset past the end", "?nextToken=5".to_string()),
        (
            "an offset past the end at the floor page size",
            format!("?pageSize={PAGE_SIZE_FLOOR}&nextToken=3"),
        ),
    ] {
        let answer = harness.list_named_shadows("paged-empty-thing", &query).await;
        assert!(
            results(case, &answer).is_empty(),
            "case {case:?}: expected an empty page, got body {}",
            String::from_utf8_lossy(&answer.body)
        );
        assert_timestamp(case, &answer);
        assert_eq!(
            next_token(&answer),
            None,
            "case {case:?}: expected no continuation off the end of an empty listing, got body {}",
            String::from_utf8_lossy(&answer.body)
        );
    }

    // A `pageSize` outside the modelled range is still refused by the model's own
    // bound, emptiness or not — the listing's success is not a licence to accept one.
    for (case, page_size) in [
        ("one below the floor", PAGE_SIZE_FLOOR - 1),
        ("one above the ceiling", PAGE_SIZE_CEILING + 1),
    ] {
        let answer = harness
            .list_named_shadows("paged-empty-thing", &format!("?pageSize={page_size}"))
            .await;
        assert_eq!(
            answer.status,
            400,
            "case {case:?} (pageSize={page_size}): expected 400, got {} with body {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
    }

    // A `nextToken` this operation never issued is still an invalid request rather than
    // an empty page.
    for (case, token) in [
        ("not a number", "not-a-token"),
        ("negative", "-1"),
        ("fractional", "1.5"),
    ] {
        let answer = harness
            .list_named_shadows("paged-empty-thing", &format!("?nextToken={token}"))
            .await;
        assert_eq!(
            answer.status,
            400,
            "case {case:?}: expected 400 for an unissued nextToken, got {} with body {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.error_code(),
            "InvalidRequestException",
            "case {case:?}: expected InvalidRequestException, got body {}",
            String::from_utf8_lossy(&answer.body)
        );
    }
}

#[tokio::test]
async fn the_listing_follows_the_store_across_a_whole_shadow_lifecycle() {
    let harness = Harness::new();
    let thing = "lifecycle-thing";

    // Before anything is written: empty, and a listing changes nothing, so a second
    // listing answers the same names.
    let before = harness.list_named_shadows(thing, "").await;
    assert!(
        results("before any write", &before).is_empty(),
        "expected an empty listing before any write, got body {}",
        String::from_utf8_lossy(&before.body)
    );
    let again = harness.list_named_shadows(thing, "").await;
    assert!(
        results("a repeated empty listing", &again).is_empty(),
        "expected a repeated empty listing to answer the same, got body {}",
        String::from_utf8_lossy(&again.body)
    );

    // Each write appears, and the classic shadow never does.
    harness.update(thing, None, state_document()).await;
    let classic_only = harness.list_named_shadows(thing, "").await;
    assert!(
        results("only a classic shadow", &classic_only).is_empty(),
        "the classic shadow is not a named shadow, got body {}",
        String::from_utf8_lossy(&classic_only.body)
    );

    for (step, name) in ["s1", "s2", "s3"].iter().enumerate() {
        harness.update(thing, Some(name), state_document()).await;
        let listed = harness.list_named_shadows(thing, "").await;
        assert_eq!(
            results("after a write", &listed),
            names(&["s1", "s2", "s3"][..=step]),
            "expected the written names so far, got body {}",
            String::from_utf8_lossy(&listed.body)
        );
    }

    // Each delete takes exactly its own name out, and the last one leaves the listing
    // empty again rather than not-found. The classic shadow stays readable throughout.
    for (step, name) in ["s2", "s1", "s3"].iter().enumerate() {
        let removed = harness.delete_shadow(thing, Some(name)).await;
        assert_eq!(
            removed.status,
            200,
            "expected the delete of {name} to be accepted, got {} with body {}",
            removed.status,
            String::from_utf8_lossy(&removed.body)
        );
        let expected: Vec<String> = match step {
            0 => names(&["s1", "s3"]),
            1 => names(&["s3"]),
            _ => Vec::new(),
        };
        let listed = harness.list_named_shadows(thing, "").await;
        assert_eq!(
            results("after a delete", &listed),
            expected,
            "expected {expected:?} after deleting {name}, got body {}",
            String::from_utf8_lossy(&listed.body)
        );
        assert_timestamp("after a delete", &listed);
        let classic = harness.get_shadow(thing, None).await;
        assert_eq!(
            classic.status,
            200,
            "expected the classic shadow to stay readable, got {} with body {}",
            classic.status,
            String::from_utf8_lossy(&classic.body)
        );
    }

    // Reading a shadow that is genuinely addressed and absent is still a 404: the
    // listing's emptiness is not a 404, and the 404 is not a listing's business.
    let missing = harness.get_shadow(thing, Some("s1")).await;
    assert_eq!(
        missing.status,
        404,
        "expected a deleted shadow to read back as 404, got {} with body {}",
        missing.status,
        String::from_utf8_lossy(&missing.body)
    );

    // And a name written after the listing went empty shows up again.
    harness.update(thing, Some("s9"), state_document()).await;
    let revived = harness.list_named_shadows(thing, "").await;
    assert_eq!(
        results("after a rewrite", &revived),
        names(&["s9"]),
        "expected the rewritten name back in the listing, got body {}",
        String::from_utf8_lossy(&revived.body)
    );
}
