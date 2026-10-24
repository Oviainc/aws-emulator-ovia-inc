// SPDX-License-Identifier: CC0-1.0

//! `TagResource` over the wire: the tags the named resource carries afterwards.
//!
//! The rule under test: tagging MERGES the request's tags into the tags already stored on
//! the resource the `ResourceArn` names — a key the resource already carries takes the
//! request's value in place (keeping its position in the stored list), a key it does not
//! carry is appended in request order, every key the request does not name keeps its
//! value, and a key the request names twice settles on its last value. The merge is
//! idempotent, and an ARN that names no stored resource is refused with
//! `ResourceNotFoundException` (HTTP 400, as `tests/all/resource_not_found.rs` pins)
//! instead of storing tags no resource owns. Like the removal rule its sibling
//! `untag_resource` suite pins, it is a rule about a RESOURCE: `ResourceArn` names a
//! `dashboard` or a `scheduled-report`, and both obey it.
//!
//! Every call goes through the real router (`POST /` with
//! `X-Amz-Target: AWSBCMDashboardsService.<Op>`, awsJson1_0), so what these tests assert
//! is what an SDK client sees: the tags a read reports come from the persisted record,
//! not from the request. The helpers and the bounded value generator are the sibling
//! `untag_resource` suite's, reused rather than duplicated.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::json;

use super::dashboard_lifecycle::{call, error_code, ok_call, router};
use super::scheduled_report_lifecycle::EXECUTION_ROLE_ARN;
use super::untag_resource::{create_dashboard_with_tags, pairs, stored_tags, tags_json, Xorshift};
use service_com_amazonaws_bcmdashboards_awsbcmdashboardsservice::state::MAX_TAGS_PER_RESOURCE;

/// One row of the merge table: the tags the resource starts with, the tags the request
/// applies, and the tags the resource must carry afterwards — in order.
struct Case {
    label: String,
    initial: Vec<(String, String)>,
    apply: Vec<(String, String)>,
    expected: Vec<(String, String)>,
}

/// The table: rows differ in how many tags are applied, whether the applied keys are
/// already stored, whether they repeat, the order they arrive in, what the key and value
/// text holds, and the modelled bounds on key length, value length and list length.
fn merge_cases() -> Vec<Case> {
    // A 128-character key, a 256-character value and a 200-entry tag list are the
    // modelled maxima (source: smithy_query(query="shape",
    // shape="ResourceTag,ResourceTagList") — key length 1..128 pattern ^[\S\s]*$, value
    // length 0..256 pattern ^[\S\s]*$, list length 0..200).
    let longest_key = "k".repeat(128);
    let longest_value = "v".repeat(256);
    let shortest_key = "k".to_string();

    let mut cases = vec![
        Case {
            label: "a key the resource does not carry is appended".to_string(),
            initial: pairs(&[("env", "prod")]),
            apply: pairs(&[("owner", "fin")]),
            expected: pairs(&[("env", "prod"), ("owner", "fin")]),
        },
        Case {
            label: "a key the resource already carries takes the new value in place".to_string(),
            initial: pairs(&[("env", "prod"), ("owner", "fin"), ("tier", "gold")]),
            apply: pairs(&[("owner", "ops")]),
            expected: pairs(&[("env", "prod"), ("owner", "ops"), ("tier", "gold")]),
        },
        Case {
            label: "updates and additions in one request".to_string(),
            initial: pairs(&[("a", "1"), ("b", "2")]),
            apply: pairs(&[("b", "22"), ("c", "3"), ("a", "11"), ("d", "4")]),
            expected: pairs(&[("a", "11"), ("b", "22"), ("c", "3"), ("d", "4")]),
        },
        Case {
            label: "no tags at all (the list's minimum length)".to_string(),
            initial: pairs(&[("env", "prod")]),
            apply: Vec::new(),
            expected: pairs(&[("env", "prod")]),
        },
        Case {
            label: "a resource that carries no tags yet".to_string(),
            initial: Vec::new(),
            apply: pairs(&[("env", "prod"), ("owner", "fin")]),
            expected: pairs(&[("env", "prod"), ("owner", "fin")]),
        },
        Case {
            label: "the same key twice in one request keeps the last value".to_string(),
            initial: pairs(&[("env", "prod")]),
            apply: pairs(&[("env", "stage"), ("env", "dev")]),
            expected: pairs(&[("env", "dev")]),
        },
        Case {
            label: "a new key named twice in one request is appended once".to_string(),
            initial: pairs(&[("env", "prod")]),
            apply: pairs(&[("owner", "fin"), ("owner", "ops")]),
            expected: pairs(&[("env", "prod"), ("owner", "ops")]),
        },
        Case {
            label: "exactly the tags the resource already carries".to_string(),
            initial: pairs(&[("env", "prod"), ("owner", "fin")]),
            apply: pairs(&[("env", "prod"), ("owner", "fin")]),
            expected: pairs(&[("env", "prod"), ("owner", "fin")]),
        },
        Case {
            label: "applied in an order the resource does not carry them in".to_string(),
            initial: pairs(&[("a", "1"), ("b", "2"), ("c", "3")]),
            apply: pairs(&[("c", "33"), ("a", "11")]),
            expected: pairs(&[("a", "11"), ("b", "2"), ("c", "33")]),
        },
        Case {
            // `^[\S\s]*$` constrains neither key nor value text, so both may hold spaces,
            // punctuation, a reserved-looking prefix or non-ASCII text, and a VALUE may be
            // empty (`ResourceTagValue` length 0..256).
            label: "keys and values the pattern allows but ASCII words do not cover"
                .to_string(),
            initial: pairs(&[("aws:cost centre", "finance ops"), ("клиент", "Ёлка")]),
            apply: pairs(&[
                ("aws:cost centre", "platform ops"),
                ("клиент", ""),
                ("emoji 🎉", "party"),
            ]),
            expected: pairs(&[
                ("aws:cost centre", "platform ops"),
                ("клиент", ""),
                ("emoji 🎉", "party"),
            ]),
        },
        Case {
            label: "an existing value replaced by the empty value the model admits"
                .to_string(),
            initial: pairs(&[("env", "prod")]),
            apply: pairs(&[("env", "")]),
            expected: pairs(&[("env", "")]),
        },
        Case {
            label: "an empty value replaced by a non-empty one".to_string(),
            initial: pairs(&[("env", "")]),
            apply: pairs(&[("env", "prod")]),
            expected: pairs(&[("env", "prod")]),
        },
        Case {
            label: "the shortest and the longest key, and the longest value".to_string(),
            initial: vec![
                (shortest_key.clone(), "short".to_string()),
                ("kept".to_string(), "yes".to_string()),
            ],
            apply: vec![
                (shortest_key.clone(), longest_value.clone()),
                (longest_key.clone(), longest_value.clone()),
            ],
            expected: vec![
                (shortest_key, longest_value.clone()),
                ("kept".to_string(), "yes".to_string()),
                (longest_key, longest_value),
            ],
        },
    ];

    // The list bound in both directions: a resource carrying 199 tags, with all 199
    // updated and one appended in a single 200-entry request, ends at the 200-tag maximum.
    let bulk: Vec<(String, String)> = (0..199)
        .map(|i| (format!("bulk-{i:03}"), format!("v{i}")))
        .collect();
    let mut applied: Vec<(String, String)> = bulk
        .iter()
        .map(|(key, _)| (key.clone(), "updated".to_string()))
        .collect();
    applied.push(("added".to_string(), "yes".to_string()));
    let mut expected: Vec<(String, String)> = bulk
        .iter()
        .map(|(key, _)| (key.clone(), "updated".to_string()))
        .collect();
    expected.push(("added".to_string(), "yes".to_string()));
    cases.push(Case {
        label: "a 200-entry request over a resource carrying 199 tags".to_string(),
        initial: bulk,
        apply: applied,
        expected,
    });

    cases
}

/// `TagResource` merges the request's tags into the stored tags, and
/// `ListTagsForResource` reports the result in the stored order.
#[tokio::test]
async fn tag_resource_merges_the_request_into_the_stored_tags() {
    for (index, case) in merge_cases().into_iter().enumerate() {
        // One router per row: a row's expectation is about its own resource only.
        let router = router();
        let name = format!("Merge Target {index}");
        let arn = create_dashboard_with_tags(&router, &name, &case.initial).await;

        let before = stored_tags(&router, &arn, &case.label).await;
        assert_eq!(
            before, case.initial,
            "case {}: the tags supplied at create time must be the ones stored, got {before:?}",
            case.label
        );

        let tagged = ok_call(
            &router,
            "TagResource",
            &json!({"resourceArn": arn, "resourceTags": tags_json(&case.apply)}),
        )
        .await;
        assert!(
            tagged.is_null() || tagged.as_object().is_some_and(serde_json::Map::is_empty),
            "case {}: TagResource models an empty response, got {tagged}",
            case.label
        );

        let after = stored_tags(&router, &arn, &case.label).await;
        assert_eq!(
            after, case.expected,
            "case {}: applying {:?} must leave {:?}, got {after:?}",
            case.label, case.apply, case.expected
        );

        // Tagging adds tags, nothing else: the dashboard still reads back unchanged.
        let fetched = ok_call(&router, "GetDashboard", &json!({"arn": arn})).await;
        assert_eq!(
            fetched["name"].as_str(),
            Some(name.as_str()),
            "case {}: tagging must leave the dashboard itself alone, got {fetched}",
            case.label
        );
    }
}

/// Repeating a merge changes nothing more, and a merge reaches only the resource the
/// request names.
#[tokio::test]
async fn tag_resource_is_idempotent_and_touches_only_the_named_resource() {
    let router = router();
    let tags = pairs(&[("env", "prod"), ("owner", "fin")]);
    let target = create_dashboard_with_tags(&router, "Tag Target", &tags).await;
    let bystander = create_dashboard_with_tags(&router, "Tag Bystander", &tags).await;

    let request = json!({
        "resourceArn": target,
        "resourceTags": tags_json(&pairs(&[("owner", "ops"), ("tier", "gold")])),
    });
    ok_call(&router, "TagResource", &request).await;
    let once = stored_tags(&router, &target, "first merge").await;
    assert_eq!(
        once,
        pairs(&[("env", "prod"), ("owner", "ops"), ("tier", "gold")]),
        "case first merge: expected owner updated in place and tier appended, got {once:?}"
    );

    // The same call again: every key it names already holds the value it carries, so it
    // succeeds and leaves the same tags in the same order.
    ok_call(&router, "TagResource", &request).await;
    let twice = stored_tags(&router, &target, "repeated merge").await;
    assert_eq!(
        twice, once,
        "case repeated merge: a repeat must change nothing, got {twice:?}"
    );

    let untouched = stored_tags(&router, &bystander, "bystander").await;
    assert_eq!(
        untouched, tags,
        "case bystander: another resource's tags must be untouched, got {untouched:?}"
    );
}

/// Tagging and untagging compose: a key removed and then re-applied comes back, appended
/// at the end rather than at the position it used to hold, and the tags a read reports
/// after each step are the persisted ones.
#[tokio::test]
async fn tags_survive_a_tag_untag_retag_round_trip() {
    let router = router();
    let arn = create_dashboard_with_tags(
        &router,
        "Round Trip",
        &pairs(&[("env", "prod"), ("owner", "fin")]),
    )
    .await;

    ok_call(
        &router,
        "UntagResource",
        &json!({"resourceArn": arn, "resourceTagKeys": ["env"]}),
    )
    .await;
    let removed = stored_tags(&router, &arn, "after the removal").await;
    assert_eq!(
        removed,
        pairs(&[("owner", "fin")]),
        "case after the removal: expected only owner, got {removed:?}"
    );

    ok_call(
        &router,
        "TagResource",
        &json!({
            "resourceArn": arn,
            "resourceTags": tags_json(&pairs(&[("env", "stage")])),
        }),
    )
    .await;
    let restored = stored_tags(&router, &arn, "after the re-tag").await;
    assert_eq!(
        restored,
        pairs(&[("owner", "fin"), ("env", "stage")]),
        "case after the re-tag: a key no longer stored is appended, got {restored:?}"
    );
}

/// The rule is about a resource, not about dashboards: `ResourceArn` names a `dashboard`
/// or a `scheduled-report`, so a report's tags merge the same way and a merge on one kind
/// leaves the other kind alone.
#[tokio::test]
async fn tag_resource_merges_into_a_scheduled_reports_tags_too() {
    let router = router();
    let dashboard =
        create_dashboard_with_tags(&router, "Report Source", &pairs(&[("env", "prod")])).await;
    let report_tags = pairs(&[("team", "finops"), ("env", "prod")]);
    let created = ok_call(
        &router,
        "CreateScheduledReport",
        &json!({
            "scheduledReport": {
                "name": "Monthly Rollup",
                "dashboardArn": dashboard,
                "scheduledReportExecutionRoleArn": EXECUTION_ROLE_ARN,
                "scheduleConfig": {"scheduleExpression": "cron(0 8 1 * ? *)"},
            },
            "resourceTags": tags_json(&report_tags),
        }),
    )
    .await;
    let report = created["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case scheduled report: no arn: {created}"))
        .to_string();

    ok_call(
        &router,
        "TagResource",
        &json!({
            "resourceArn": report,
            "resourceTags": tags_json(&pairs(&[("env", "stage"), ("blank", "")])),
        }),
    )
    .await;
    let after = stored_tags(&router, &report, "scheduled report after").await;
    assert_eq!(
        after,
        pairs(&[("team", "finops"), ("env", "stage"), ("blank", "")]),
        "case scheduled report after: expected env updated in place and blank appended, got {after:?}"
    );

    // The dashboard the report points at keeps its own `env` tag: the merge reached the
    // report alone.
    let dashboard_tags = stored_tags(&router, &dashboard, "the source dashboard").await;
    assert_eq!(
        dashboard_tags,
        pairs(&[("env", "prod")]),
        "case the source dashboard: a merge on the report must leave it alone, got {dashboard_tags:?}"
    );

    // The report itself survives tagging.
    let fetched = ok_call(&router, "GetScheduledReport", &json!({"arn": report})).await;
    assert_eq!(
        fetched["scheduledReport"]["name"].as_str(),
        Some("Monthly Rollup"),
        "case scheduled report: the report must survive tagging, got {fetched}"
    );
}

/// One row of the bounds table: the tags a request carries, and whether the model's own
/// constraints admit it.
struct Bound {
    label: String,
    tags: Vec<(String, String)>,
    admitted: bool,
}

/// The modelled constraints on what a merge may carry, at each published bound and one
/// past it (source: smithy_query(query="shape", shape="ResourceTag,ResourceTagList") —
/// key length 1..128, value length 0..256, list length 0..200).
#[tokio::test]
async fn tag_resource_honours_the_modelled_tag_constraints() {
    let cases: Vec<Bound> = vec![
        Bound {
            label: "an empty list (the list minimum)".to_string(),
            tags: Vec::new(),
            admitted: true,
        },
        Bound {
            label: "a one-character key (the key minimum)".to_string(),
            tags: vec![("k".to_string(), "v".to_string())],
            admitted: true,
        },
        Bound {
            label: "a 128-character key (the key maximum)".to_string(),
            tags: vec![("k".repeat(128), "v".to_string())],
            admitted: true,
        },
        Bound {
            label: "a 129-character key (one past the key maximum)".to_string(),
            tags: vec![("k".repeat(129), "v".to_string())],
            admitted: false,
        },
        Bound {
            label: "an empty key (one short of the key minimum)".to_string(),
            tags: vec![(String::new(), "v".to_string())],
            admitted: false,
        },
        Bound {
            label: "an empty value (the value minimum)".to_string(),
            tags: vec![("empty-value".to_string(), String::new())],
            admitted: true,
        },
        Bound {
            label: "a 256-character value (the value maximum)".to_string(),
            tags: vec![("long-value".to_string(), "v".repeat(256))],
            admitted: true,
        },
        Bound {
            label: "a 257-character value (one past the value maximum)".to_string(),
            tags: vec![("too-long-value".to_string(), "v".repeat(257))],
            admitted: false,
        },
        Bound {
            label: "200 tags (the list maximum)".to_string(),
            tags: (0..200)
                .map(|i| (format!("bound-{i:03}"), format!("v{i}")))
                .collect(),
            admitted: true,
        },
        Bound {
            label: "201 tags (one past the list maximum)".to_string(),
            tags: (0..201)
                .map(|i| (format!("over-{i:03}"), format!("v{i}")))
                .collect(),
            admitted: false,
        },
    ];

    let router = router();

    for (index, case) in cases.iter().enumerate() {
        let label = &case.label;
        // A FRESH, untagged resource per row, so what a row asserts is the model's own
        // constraint on that request and not the size the earlier rows left behind (the
        // STORED list has its own ceiling, which
        // `a_merge_past_the_stored_tag_ceiling_is_refused_and_stores_nothing` covers).
        let arn =
            create_dashboard_with_tags(&router, &format!("Bounds Row {index}"), &[]).await;
        let (status, answered) = call(
            &router,
            "TagResource",
            &json!({"resourceArn": arn, "resourceTags": tags_json(&case.tags)}),
        )
        .await;
        let stored = stored_tags(&router, &arn, label).await;
        if case.admitted {
            assert_eq!(
                status, 200,
                "case {label}: the model admits this request, got {status}: {answered}"
            );
            assert_eq!(
                stored, case.tags,
                "case {label}: an admitted request must store exactly its tags, got {} tags",
                stored.len()
            );
        } else {
            assert_eq!(
                status, 400,
                "case {label}: the model refuses this request, got {status}: {answered}"
            );
            assert!(
                error_code(&answered).contains("ValidationException"),
                "case {label}: expected ValidationException, got {}",
                error_code(&answered)
            );
            assert!(
                stored.is_empty(),
                "case {label}: a refused request must store nothing, got {stored:?}"
            );
        }
    }
}

/// The stored tag list has its OWN ceiling: the modelled `resourceTags` length bounds one
/// request, not the list a merge grows, so a merge that would carry a resource past
/// `MAX_TAGS_PER_RESOURCE` is refused with `ValidationException` and stores nothing —
/// while a merge that only updates keys the resource already carries stays admitted at
/// the ceiling.
#[tokio::test]
async fn a_merge_past_the_stored_tag_ceiling_is_refused_and_stores_nothing() {
    let router = router();

    // Half the ceiling at create time, then the other half merged in: the ceiling is
    // reachable from the merge path, not only from a create.
    let half = MAX_TAGS_PER_RESOURCE / 2;
    let initial: Vec<(String, String)> = (0..half)
        .map(|i| (format!("create-{i:03}"), format!("v{i}")))
        .collect();
    let arn = create_dashboard_with_tags(&router, "Tag Ceiling", &initial).await;

    let filling: Vec<(String, String)> = (0..MAX_TAGS_PER_RESOURCE - half)
        .map(|i| (format!("merge-{i:03}"), format!("v{i}")))
        .collect();
    ok_call(
        &router,
        "TagResource",
        &json!({"resourceArn": arn, "resourceTags": tags_json(&filling)}),
    )
    .await;
    let at_the_ceiling = stored_tags(&router, &arn, "at the ceiling").await;
    assert_eq!(
        at_the_ceiling.len(),
        MAX_TAGS_PER_RESOURCE,
        "case at the ceiling: expected {MAX_TAGS_PER_RESOURCE} stored tags, got {}",
        at_the_ceiling.len()
    );

    // One further NEW key would carry the stored list past the ceiling: refused.
    let (status, answered) = call(
        &router,
        "TagResource",
        &json!({
            "resourceArn": arn,
            "resourceTags": tags_json(&pairs(&[("one-too-many", "yes")])),
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case one past the ceiling: expected 400, got {status}: {answered}"
    );
    assert!(
        error_code(&answered).contains("ValidationException"),
        "case one past the ceiling: expected ValidationException, got {}",
        error_code(&answered)
    );
    let after_refusal = stored_tags(&router, &arn, "after the refusal").await;
    assert_eq!(
        after_refusal, at_the_ceiling,
        "case one past the ceiling: a refused merge must store nothing, got {} tags",
        after_refusal.len()
    );

    // A merge that names only keys the resource already carries does not grow the list,
    // so it is admitted at the ceiling and updates them in place.
    ok_call(
        &router,
        "TagResource",
        &json!({
            "resourceArn": arn,
            "resourceTags": tags_json(&pairs(&[("create-000", "updated")])),
        }),
    )
    .await;
    let after_update = stored_tags(&router, &arn, "an update at the ceiling").await;
    assert_eq!(
        after_update.len(),
        MAX_TAGS_PER_RESOURCE,
        "case an update at the ceiling: the list must not grow, got {}",
        after_update.len()
    );
    assert_eq!(
        after_update.first().cloned(),
        Some(("create-000".to_string(), "updated".to_string())),
        "case an update at the ceiling: expected create-000 updated in place, got {:?}",
        after_update.first()
    );

    // Freeing a slot makes room again: the refused key lands once a key is removed.
    ok_call(
        &router,
        "UntagResource",
        &json!({"resourceArn": arn, "resourceTagKeys": ["create-001"]}),
    )
    .await;
    ok_call(
        &router,
        "TagResource",
        &json!({
            "resourceArn": arn,
            "resourceTags": tags_json(&pairs(&[("one-too-many", "yes")])),
        }),
    )
    .await;
    let after_room = stored_tags(&router, &arn, "after making room").await;
    assert_eq!(
        after_room.len(),
        MAX_TAGS_PER_RESOURCE,
        "case after making room: expected {MAX_TAGS_PER_RESOURCE} stored tags, got {}",
        after_room.len()
    );
    assert_eq!(
        after_room.last().cloned(),
        Some(("one-too-many".to_string(), "yes".to_string())),
        "case after making room: expected the new key appended, got {:?}",
        after_room.last()
    );
}

/// A read between two merges changes nothing: two reads agree, and the second merge sees
/// what the first one left.
#[tokio::test]
async fn reading_the_tags_between_merges_changes_nothing() {
    let router = router();
    let arn = create_dashboard_with_tags(&router, "Read Twice", &pairs(&[("env", "prod")])).await;

    let request = json!({
        "resourceArn": arn,
        "resourceTags": tags_json(&pairs(&[("owner", "fin")])),
    });
    ok_call(&router, "TagResource", &request).await;

    let first = stored_tags(&router, &arn, "first read").await;
    let second = stored_tags(&router, &arn, "second read").await;
    assert_eq!(
        first, second,
        "case repeated read: a read must not change what a later caller sees, got {second:?} after {first:?}"
    );

    ok_call(
        &router,
        "TagResource",
        &json!({
            "resourceArn": arn,
            "resourceTags": tags_json(&pairs(&[("owner", "ops")])),
        }),
    )
    .await;
    let third = stored_tags(&router, &arn, "after the second merge").await;
    assert_eq!(
        third,
        pairs(&[("env", "prod"), ("owner", "ops")]),
        "case after the second merge: expected owner updated in place, got {third:?}"
    );
}

/// The merge rule as a property, over generated stored tag sets and generated requests:
/// whatever the stored tags and whatever the request, afterwards every key is stored
/// once, every key the request names holds the LAST value the request gave it, every
/// stored key the request did not name keeps its value, the stored keys keep their order
/// with the request's new keys appended in first-appearance order, and repeating the
/// request changes nothing more.
///
/// The run is bounded (32 cases), seeded from the constant below (so a failure is
/// reproducible and every assertion names the seed and the case) and persists nothing to
/// disk. The generator is the sibling `untag_resource` suite's, which explains there why
/// it is 15 lines of arithmetic rather than a new `[dev-dependencies]` entry.
#[tokio::test]
async fn merging_generated_tag_sets_obeys_the_merge_invariants() {
    const SEED: u64 = 0x7a61_c0de;
    const CASES: usize = 32;

    let mut generator = Xorshift::new(SEED);
    // The three shapes the generated cases must actually cover, so a degenerate
    // generator cannot make this property vacuous.
    let mut saw_an_update = 0_usize;
    let mut saw_an_addition = 0_usize;
    let mut saw_an_untouched_tag = 0_usize;
    for case in 0..CASES {
        // A `BTreeMap` so a generated stored tag set carries each key once, as a
        // resource's tags do; the create request submits it in that (sorted) order.
        let mut generated: BTreeMap<String, String> = BTreeMap::new();
        for _ in 0..generator.below(6) {
            let key = generator.key();
            let value = generator.value();
            generated.insert(key, value);
        }
        let initial: Vec<(String, String)> = generated.into_iter().collect();

        // The request MAY repeat a key, so the last-value rule is exercised.
        let apply: Vec<(String, String)> = (0..generator.below(6))
            .map(|_| (generator.key(), generator.value()))
            .collect();

        let stored_keys: Vec<String> = initial.iter().map(|(key, _)| key.clone()).collect();
        let named: BTreeSet<&str> = apply.iter().map(|(key, _)| key.as_str()).collect();
        // The value each named key must end up with: the LAST one the request gave it.
        let mut last_value: BTreeMap<&str, &str> = BTreeMap::new();
        for (key, value) in &apply {
            last_value.insert(key.as_str(), value.as_str());
        }
        // The keys the request adds, in first-appearance order.
        let mut added: Vec<&str> = Vec::new();
        for (key, _) in &apply {
            if !stored_keys.iter().any(|stored| stored == key)
                && !added.contains(&key.as_str())
            {
                added.push(key.as_str());
            }
        }
        if named.iter().any(|key| stored_keys.iter().any(|s| s == key)) {
            saw_an_update += 1;
        }
        if !added.is_empty() {
            saw_an_addition += 1;
        }
        if stored_keys
            .iter()
            .any(|key| !named.contains(key.as_str()))
        {
            saw_an_untouched_tag += 1;
        }

        let router = router();
        let name = format!("Generated Merge {case}");
        let arn = create_dashboard_with_tags(&router, &name, &initial).await;
        let request = json!({"resourceArn": arn, "resourceTags": tags_json(&apply)});

        ok_call(&router, "TagResource", &request).await;
        let after = stored_tags(&router, &arn, &name).await;
        let label =
            format!("seed {SEED:#x} #{case} stored {initial:?} applied {apply:?}");

        // Every key is stored once.
        let distinct: BTreeSet<&str> = after.iter().map(|(key, _)| key.as_str()).collect();
        assert_eq!(
            distinct.len(),
            after.len(),
            "case {label}: a key must be stored once, got {after:?}"
        );

        // Every key the request named holds the last value the request gave it.
        for (key, value) in &last_value {
            let found = after
                .iter()
                .find(|(stored_key, _)| stored_key == key)
                .map(|(_, stored_value)| stored_value.as_str());
            assert_eq!(
                found,
                Some(*value),
                "case {label}: expected {key:?} to hold {value:?}, got {found:?} in {after:?}"
            );
        }

        // Every stored key the request did not name keeps its value.
        for (key, value) in &initial {
            if named.contains(key.as_str()) {
                continue;
            }
            let found = after
                .iter()
                .find(|(stored_key, _)| stored_key == key)
                .map(|(_, stored_value)| stored_value.as_str());
            assert_eq!(
                found,
                Some(value.as_str()),
                "case {label}: expected the untouched {key:?} to keep {value:?}, got {found:?} in {after:?}"
            );
        }

        // Order: the stored keys first, in their stored order, then the new keys in the
        // order the request first named them.
        let expected_keys: Vec<&str> = stored_keys
            .iter()
            .map(String::as_str)
            .chain(added.iter().copied())
            .collect();
        let actual_keys: Vec<&str> = after.iter().map(|(key, _)| key.as_str()).collect();
        assert_eq!(
            actual_keys, expected_keys,
            "case {label}: expected the key order {expected_keys:?}, got {actual_keys:?}"
        );

        // Idempotence: the same request again changes nothing more.
        ok_call(&router, "TagResource", &request).await;
        let after_repeat = stored_tags(&router, &arn, &name).await;
        assert_eq!(
            after_repeat, after,
            "case {label}: a repeat must change nothing, got {after_repeat:?} after {after:?}"
        );
    }

    assert!(
        saw_an_update > 0 && saw_an_addition > 0 && saw_an_untouched_tag > 0,
        "case seed {SEED:#x}: the {CASES} generated cases must cover an update, an addition and an untouched tag, got {saw_an_update} / {saw_an_addition} / {saw_an_untouched_tag}"
    );
}
