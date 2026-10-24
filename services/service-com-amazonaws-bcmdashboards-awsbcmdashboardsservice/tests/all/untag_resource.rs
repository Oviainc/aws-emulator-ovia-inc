// SPDX-License-Identifier: CC0-1.0

//! `UntagResource` over the wire: the tags the named resource keeps afterwards.
//!
//! The rule under test: removing tags from a resource removes exactly the tag keys the
//! request names and nothing else — a key the resource does not carry is a no-op, so the
//! call is idempotent, every tag whose key the request did not name keeps its value and
//! its position in the stored list, and an ARN that names no stored resource is refused
//! with `ResourceNotFoundException` (HTTP 400, as `tests/all/resource_not_found.rs` pins)
//! instead of silently succeeding. It is a
//! rule about a RESOURCE: `ResourceArn` names a `dashboard` or a `scheduled-report`, and
//! both obey it.
//!
//! Every call goes through the real router (`POST /` with
//! `X-Amz-Target: AWSBCMDashboardsService.<Op>`, awsJson1_0), so what these tests assert
//! is what an SDK client sees: the tags a read reports come from the persisted record,
//! not from the request. The sibling `resource_tags` suite covers the reader itself.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::dashboard_lifecycle::{call, error_code, ok_call, router};

/// A well-formed `ServiceRoleArn` (pattern
/// `^arn:aws[-a-z0-9]*:iam::[0-9]{12}:role/[a-zA-Z0-9+=,.@_/-]+$`, source:
/// `smithy_query(query="operation", operation="CreateScheduledReport")`), for the rows
/// that tag the OTHER kind of resource a `ResourceArn` names.
const EXECUTION_ROLE_ARN: &str = "arn:aws:iam::123456789012:role/BCMDashboardsReportRole";

/// Borrowed literals as owned `(key, value)` pairs.
pub(crate) fn pairs(entries: &[(&str, &str)]) -> Vec<(String, String)> {
    entries
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// A `resourceTagKeys` list as owned strings.
fn key_list(keys: &[&str]) -> Vec<String> {
    keys.iter().map(|key| (*key).to_string()).collect()
}

/// `(key, value)` pairs rendered as the modelled `ResourceTagList` wire shape.
pub(crate) fn tags_json(tags: &[(String, String)]) -> Value {
    Value::Array(
        tags.iter()
            .map(|(key, value)| json!({"key": key, "value": value}))
            .collect(),
    )
}

/// Create a dashboard carrying `tags` and return the ARN it was stored under.
pub(crate) async fn create_dashboard_with_tags(
    router: &MultiServiceRouter,
    name: &str,
    tags: &[(String, String)],
) -> String {
    let created = ok_call(
        router,
        "CreateDashboard",
        &json!({"name": name, "widgets": [], "resourceTags": tags_json(tags)}),
    )
    .await;
    created["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name}: CreateDashboard returned no arn: {created}"))
        .to_string()
}

/// The tags `ListTagsForResource` reports for `arn`, in the order it reported them.
pub(crate) async fn stored_tags(
    router: &MultiServiceRouter,
    arn: &str,
    case: &str,
) -> Vec<(String, String)> {
    let listed = ok_call(router, "ListTagsForResource", &json!({"resourceArn": arn})).await;
    listed["resourceTags"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: ListTagsForResource reported no list: {listed}"))
        .iter()
        .map(|tag| {
            (
                tag["key"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case}: a reported tag carries no key: {tag}"))
                    .to_string(),
                tag["value"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case}: a reported tag carries no value: {tag}"))
                    .to_string(),
            )
        })
        .collect()
}

/// One row of the removal table: the tags the resource starts with, the keys the request
/// removes, and the tags the resource must carry afterwards — in order.
struct Case {
    label: String,
    initial: Vec<(String, String)>,
    remove: Vec<String>,
    expected: Vec<(String, String)>,
}

/// The table: rows differ in how many keys are removed, whether the named keys are
/// present at all, whether they repeat, the order they arrive in, what the key text
/// holds, and the modelled bounds on key length and list length.
fn untag_cases() -> Vec<Case> {
    // A 128-character tag key and a 200-entry tag list are the modelled maxima
    // (source: smithy_query(query="shape", shape="ResourceTagKey,ResourceTagKeyList,
    // ResourceTagList") — key length 1..128, pattern ^[\S\s]*$; list length 0..200).
    let longest_key = "k".repeat(128);
    let shortest_key = "k".to_string();
    let bulk: Vec<(String, String)> = (0..199)
        .map(|i| (format!("bulk-{i:03}"), format!("v{i}")))
        .collect();

    let mut cases = vec![
        Case {
            label: "one of three keys".to_string(),
            initial: pairs(&[("env", "prod"), ("owner", "fin"), ("tier", "gold")]),
            remove: key_list(&["owner"]),
            expected: pairs(&[("env", "prod"), ("tier", "gold")]),
        },
        Case {
            label: "every key the resource carries".to_string(),
            initial: pairs(&[("env", "prod"), ("owner", "fin")]),
            remove: key_list(&["env", "owner"]),
            expected: Vec::new(),
        },
        Case {
            label: "no keys at all (the list's minimum length)".to_string(),
            initial: pairs(&[("env", "prod")]),
            remove: Vec::new(),
            expected: pairs(&[("env", "prod")]),
        },
        Case {
            label: "only keys the resource does not carry".to_string(),
            initial: pairs(&[("env", "prod")]),
            remove: key_list(&["owner", "tier"]),
            expected: pairs(&[("env", "prod")]),
        },
        Case {
            label: "a present and an absent key together".to_string(),
            initial: pairs(&[("env", "prod"), ("owner", "fin")]),
            remove: key_list(&["owner", "never-set"]),
            expected: pairs(&[("env", "prod")]),
        },
        Case {
            label: "the same key twice in one request".to_string(),
            initial: pairs(&[("env", "prod"), ("owner", "fin")]),
            remove: key_list(&["env", "env"]),
            expected: pairs(&[("owner", "fin")]),
        },
        Case {
            label: "keys named in an order the resource does not carry them in".to_string(),
            initial: pairs(&[("a", "1"), ("b", "2"), ("c", "3")]),
            remove: key_list(&["c", "a"]),
            expected: pairs(&[("b", "2")]),
        },
        Case {
            label: "a resource with no tags".to_string(),
            initial: Vec::new(),
            remove: key_list(&["env"]),
            expected: Vec::new(),
        },
        Case {
            // `^[\S\s]*$` constrains the key text to nothing at all, so a key may hold
            // spaces, punctuation, a reserved-looking prefix or non-ASCII text, and a
            // VALUE may be empty (`ResourceTagValue` length 0..256).
            label: "keys and values the pattern allows but ASCII words do not cover".to_string(),
            initial: pairs(&[
                ("aws:cost centre", "finance ops"),
                ("клиент", "Ёлка"),
                ("blank-value", ""),
            ]),
            remove: key_list(&["aws:cost centre", "никто"]),
            expected: pairs(&[("клиент", "Ёлка"), ("blank-value", "")]),
        },
        Case {
            label: "the shortest and the longest key the model allows".to_string(),
            initial: vec![
                (shortest_key.clone(), "short".to_string()),
                (longest_key.clone(), "long".to_string()),
                ("kept".to_string(), "yes".to_string()),
            ],
            remove: vec![shortest_key, longest_key],
            expected: pairs(&[("kept", "yes")]),
        },
    ];

    // The list bound: a resource at the 200-tag maximum, with 199 keys removed in one
    // call, leaves exactly the one key the request did not name.
    let mut at_the_bound = bulk.clone();
    at_the_bound.push(("kept".to_string(), "yes".to_string()));
    cases.push(Case {
        label: "199 keys removed from a resource at the 200-tag maximum".to_string(),
        initial: at_the_bound,
        remove: bulk.iter().map(|(key, _)| key.clone()).collect(),
        expected: pairs(&[("kept", "yes")]),
    });

    cases
}

/// `UntagResource` removes exactly the keys the request names, and `ListTagsForResource`
/// reports what the resource still carries, in the order it was stored.
#[tokio::test]
async fn untag_resource_removes_exactly_the_named_keys() {
    for (index, case) in untag_cases().into_iter().enumerate() {
        // One router per row: a row's expectation is about its own resource only.
        let router = router();
        let name = format!("Tagged Dashboard {index}");
        let arn = create_dashboard_with_tags(&router, &name, &case.initial).await;

        let before = stored_tags(&router, &arn, &case.label).await;
        assert_eq!(
            before, case.initial,
            "case {}: the tags supplied at create time must be the ones stored, got {before:?}",
            case.label
        );

        let untagged = ok_call(
            &router,
            "UntagResource",
            &json!({"resourceArn": arn, "resourceTagKeys": case.remove}),
        )
        .await;
        assert!(
            untagged.is_null() || untagged.as_object().is_some_and(serde_json::Map::is_empty),
            "case {}: UntagResource models an empty response, got {untagged}",
            case.label
        );

        let after = stored_tags(&router, &arn, &case.label).await;
        assert_eq!(
            after, case.expected,
            "case {}: removing {:?} must leave {:?}, got {after:?}",
            case.label, case.remove, case.expected
        );

        // Removing tags removes tags, not the resource: the dashboard still reads back.
        let fetched = ok_call(&router, "GetDashboard", &json!({"arn": arn})).await;
        assert_eq!(
            fetched["name"].as_str(),
            Some(name.as_str()),
            "case {}: the dashboard must survive untagging, got {fetched}",
            case.label
        );
    }
}

/// Repeating a removal changes nothing more, and a removal reaches only the resource the
/// request names.
#[tokio::test]
async fn untag_resource_is_idempotent_and_touches_only_the_named_resource() {
    let router = router();
    let tags = pairs(&[("env", "prod"), ("owner", "fin"), ("tier", "gold")]);
    let target = create_dashboard_with_tags(&router, "Untag Target", &tags).await;
    let bystander = create_dashboard_with_tags(&router, "Untag Bystander", &tags).await;

    let request = json!({"resourceArn": target, "resourceTagKeys": ["owner", "absent"]});
    ok_call(&router, "UntagResource", &request).await;
    let once = stored_tags(&router, &target, "first removal").await;
    assert_eq!(
        once,
        pairs(&[("env", "prod"), ("tier", "gold")]),
        "case first removal: expected env and tier to remain, got {once:?}"
    );

    // The same call again: the named keys are already gone, so it succeeds and leaves
    // the same tags.
    ok_call(&router, "UntagResource", &request).await;
    let twice = stored_tags(&router, &target, "repeated removal").await;
    assert_eq!(
        twice, once,
        "case repeated removal: a repeat must change nothing, got {twice:?}"
    );

    let untouched = stored_tags(&router, &bystander, "bystander").await;
    assert_eq!(
        untouched, tags,
        "case bystander: another resource's tags must be untouched, got {untouched:?}"
    );
}

/// A read between two removals changes nothing either: two reads agree, and the second
/// removal sees what the first one left.
#[tokio::test]
async fn reading_the_tags_between_removals_changes_nothing() {
    let router = router();
    let tags = pairs(&[("env", "prod"), ("owner", "fin")]);
    let arn = create_dashboard_with_tags(&router, "Read Twice", &tags).await;

    let first = stored_tags(&router, &arn, "first read").await;
    let second = stored_tags(&router, &arn, "second read").await;
    assert_eq!(
        first, second,
        "case repeated read: a read must not change what a later caller sees, got {second:?} after {first:?}"
    );
    assert_eq!(
        second, tags,
        "case repeated read: expected the stored tags, got {second:?}"
    );

    ok_call(
        &router,
        "UntagResource",
        &json!({"resourceArn": arn, "resourceTagKeys": ["env"]}),
    )
    .await;
    let after_removal = stored_tags(&router, &arn, "after removal").await;
    assert_eq!(
        after_removal,
        pairs(&[("owner", "fin")]),
        "case after removal: expected only owner, got {after_removal:?}"
    );
}

/// An ARN that names no stored resource is a not-found; an ARN outside the modelled
/// `ResourceArn` is a validation failure.
#[tokio::test]
async fn untagging_an_unknown_resource_is_refused() {
    let router = router();
    let live = create_dashboard_with_tags(&router, "Still Here", &pairs(&[("env", "prod")])).await;
    let doomed = create_dashboard_with_tags(&router, "Soon Gone", &pairs(&[("env", "prod")])).await;
    ok_call(&router, "DeleteDashboard", &json!({"arn": doomed})).await;

    // Each row is a well-formed `ResourceArn` that no stored resource answers to:
    // `ResourceArn` also admits `scheduled-report/...` and the wildcard resource id,
    // and no resource of either kind is stored under these.
    let unknown: Vec<(&str, String)> = vec![
        (
            "a dashboard that was never created",
            "arn:aws:bcm-dashboards::000000000000:dashboard/never-created".to_string(),
        ),
        ("a dashboard that was deleted", doomed.clone()),
        (
            "a scheduled report that was never created",
            "arn:aws:bcm-dashboards::000000000000:scheduled-report/report-1".to_string(),
        ),
        (
            "the wildcard resource id",
            "arn:aws:bcm-dashboards::000000000000:dashboard/*".to_string(),
        ),
        (
            "another partition",
            "arn:aws-us-gov:bcm-dashboards::000000000000:dashboard/elsewhere".to_string(),
        ),
    ];

    for (label, arn) in &unknown {
        let (status, answered) = call(
            &router,
            "UntagResource",
            &json!({"resourceArn": arn, "resourceTagKeys": ["env"]}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {label}: expected 400, got {status}: {answered}"
        );
        assert!(
            error_code(&answered).contains("ResourceNotFoundException"),
            "case {label}: expected ResourceNotFoundException, got {}",
            error_code(&answered)
        );
    }

    // A refused call stored nothing and removed nothing.
    let survivor = stored_tags(&router, &live, "the live resource").await;
    assert_eq!(
        survivor,
        pairs(&[("env", "prod")]),
        "case the live resource: the refusals must leave it alone, got {survivor:?}"
    );

    // Each row breaks `ResourceArn`'s pattern or its length in a different way
    // (source: smithy_query(query="operation", operation="UntagResource") — length
    // 20..2048, pattern
    // ^arn:aws[-a-z0-9]*:bcm-dashboards::[0-9]{12}:(dashboard|scheduled-report)/(\*|[-a-z0-9]+)$).
    let malformed: &[(&str, &str)] = &[
        ("not an arn at all", "not-an-arn"),
        (
            "an account id that is not 12 digits",
            "arn:aws:bcm-dashboards::12345:dashboard/short-account",
        ),
        (
            "a region segment, which this service's ARNs do not carry",
            "arn:aws:bcm-dashboards:us-east-1:000000000000:dashboard/with-region",
        ),
        (
            "another service's resource type",
            "arn:aws:bcm-dashboards::000000000000:widget/not-a-dashboard",
        ),
        (
            "an uppercase resource id",
            "arn:aws:bcm-dashboards::000000000000:dashboard/NotLowercase",
        ),
        ("shorter than the 20-character minimum", "arn:aws:bcm-d:x"),
    ];
    for (label, arn) in malformed {
        let (status, answered) = call(
            &router,
            "UntagResource",
            &json!({"resourceArn": arn, "resourceTagKeys": ["env"]}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {label}: {arn:?} must be refused as invalid, got {status}: {answered}"
        );
        assert!(
            error_code(&answered).contains("ValidationException"),
            "case {label}: expected ValidationException, got {}",
            error_code(&answered)
        );
    }
}

/// The rule is about a resource, not about dashboards: `ResourceArn` names a `dashboard`
/// or a `scheduled-report`, so a report's tags obey the same removal rule and a removal
/// on one kind leaves the other kind alone.
#[tokio::test]
async fn untag_resource_removes_the_named_keys_from_a_scheduled_report_too() {
    let router = router();
    let dashboard =
        create_dashboard_with_tags(&router, "Report Source", &pairs(&[("env", "prod")])).await;
    let report_tags = pairs(&[("team", "finops"), ("env", "prod"), ("blank", "")]);
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

    let before = stored_tags(&router, &report, "scheduled report before").await;
    assert_eq!(
        before, report_tags,
        "case scheduled report before: expected the tags supplied at create time, got {before:?}"
    );

    ok_call(
        &router,
        "UntagResource",
        &json!({"resourceArn": report, "resourceTagKeys": ["env", "never-set"]}),
    )
    .await;
    let after = stored_tags(&router, &report, "scheduled report after").await;
    assert_eq!(
        after,
        pairs(&[("team", "finops"), ("blank", "")]),
        "case scheduled report after: expected team and blank to remain, got {after:?}"
    );

    // The dashboard the report points at carries its own `env` tag, which this removal
    // did not name.
    let dashboard_tags = stored_tags(&router, &dashboard, "the source dashboard").await;
    assert_eq!(
        dashboard_tags,
        pairs(&[("env", "prod")]),
        "case the source dashboard: a removal on the report must leave it alone, got {dashboard_tags:?}"
    );

    // The report itself survives untagging.
    let fetched = ok_call(&router, "GetScheduledReport", &json!({"arn": report})).await;
    assert_eq!(
        fetched["scheduledReport"]["name"].as_str(),
        Some("Monthly Rollup"),
        "case scheduled report: the report must survive untagging, got {fetched}"
    );
}

/// The modelled constraints on what a removal may name, at each published bound and one
/// past it (source: smithy_query(query="shape",
/// shape="ResourceTagKey,ResourceTagKeyList") — key length 1..128, list length 0..200).
#[tokio::test]
async fn untag_resource_honours_the_modelled_key_constraints() {
    let router = router();
    let arn = create_dashboard_with_tags(&router, "Bounds", &pairs(&[("env", "prod")])).await;

    // (label, the key list, whether the model admits it)
    let cases: Vec<(String, Vec<String>, bool)> = vec![
        ("an empty list (the minimum)".to_string(), Vec::new(), true),
        (
            "a one-character key (the key minimum)".to_string(),
            vec!["k".to_string()],
            true,
        ),
        (
            "a 128-character key (the key maximum)".to_string(),
            vec!["k".repeat(128)],
            true,
        ),
        (
            "a 129-character key (one past the key maximum)".to_string(),
            vec!["k".repeat(129)],
            false,
        ),
        (
            "an empty key (one short of the key minimum)".to_string(),
            vec![String::new()],
            false,
        ),
        (
            "200 keys (the list maximum)".to_string(),
            (0..200).map(|i| format!("k{i:03}")).collect(),
            true,
        ),
        (
            "201 keys (one past the list maximum)".to_string(),
            (0..201).map(|i| format!("k{i:03}")).collect(),
            false,
        ),
    ];

    for (label, keys, admitted) in &cases {
        let (status, answered) = call(
            &router,
            "UntagResource",
            &json!({"resourceArn": arn, "resourceTagKeys": keys}),
        )
        .await;
        if *admitted {
            assert_eq!(
                status, 200,
                "case {label}: the model admits this request, got {status}: {answered}"
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
        }
    }

    // None of those rows named `env`, so the resource still carries it: a refusal
    // removes nothing, and an admitted removal removes only what it names.
    let remaining = stored_tags(&router, &arn, "after the bound rows").await;
    assert_eq!(
        remaining,
        pairs(&[("env", "prod")]),
        "case after the bound rows: expected env to remain, got {remaining:?}"
    );
}

/// A deterministic value generator for the invariant test below: xorshift64* seeded
/// from one constant, so every generated case is reproducible from the seed the
/// assertions print and nothing is written to disk.
///
/// A generator crate would be the idiomatic source of these cases, but a new
/// `[dev-dependencies]` entry also edits the workspace `Cargo.lock` — the one file
/// every other landing change edits too — which is what kept this change from merging.
/// The generator is therefore 15 lines of arithmetic here rather than a dependency.
pub(crate) struct Xorshift(u64);

impl Xorshift {
    /// Seed the generator. The state is forced odd so it can never be the zero state,
    /// which xorshift maps to itself.
    pub(crate) fn new(seed: u64) -> Self {
        Self(seed | 1)
    }

    fn next_u64(&mut self) -> u64 {
        let mut state = self.0;
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        self.0 = state;
        state.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    /// A value in `0..bound`, or 0 when `bound` is 0.
    pub(crate) fn below(&mut self, bound: usize) -> usize {
        let Ok(bound) = u64::try_from(bound) else {
            return 0;
        };
        if bound == 0 {
            return 0;
        }
        usize::try_from(self.next_u64() % bound).unwrap_or(0)
    }

    /// A key of 1..=3 characters drawn from a three-letter alphabet, so a generated
    /// removal list overlaps the generated tag keys partly, wholly or not at all — the
    /// three cases the table above spells out by hand — and may repeat a key.
    pub(crate) fn key(&mut self) -> String {
        const ALPHABET: &[u8] = b"abc";
        let length = 1 + self.below(ALPHABET.len());
        (0..length)
            .map(|_| char::from(ALPHABET[self.below(ALPHABET.len())]))
            .collect()
    }

    /// A tag value of 0..=4 digits — the empty string included, which
    /// `ResourceTagValue` (length 0..256) admits.
    pub(crate) fn value(&mut self) -> String {
        const DIGITS: &[u8] = b"0123456789";
        let length = self.below(5);
        (0..length)
            .map(|_| char::from(DIGITS[self.below(DIGITS.len())]))
            .collect()
    }
}

/// The removal rule as a property, over generated tag sets and generated key lists:
/// whatever the tags and whatever the keys, the resource ends up carrying exactly the
/// stored tags minus the named keys, in their stored order, and repeating the request
/// changes nothing more.
///
/// The run is bounded (32 cases), seeded from the constant below (so a failure is
/// reproducible and every assertion names the seed and the case) and persists nothing
/// to disk.
#[tokio::test]
async fn removing_generated_key_sets_leaves_exactly_the_difference() {
    const SEED: u64 = 0x5ea1_1ced;
    const CASES: usize = 32;

    let mut generator = Xorshift::new(SEED);
    // The three shapes the generated cases must actually cover, so a degenerate
    // generator cannot make this property vacuous.
    let mut saw_a_removal = 0_usize;
    let mut saw_an_absent_key = 0_usize;
    let mut saw_a_kept_tag = 0_usize;
    for case in 0..CASES {
        // A `BTreeMap` so a generated tag set carries each key once, as a resource's
        // tags do; the create request submits it in that (sorted) order.
        let mut generated: BTreeMap<String, String> = BTreeMap::new();
        let tag_count = generator.below(6);
        for _ in 0..tag_count {
            let key = generator.key();
            let value = generator.value();
            generated.insert(key, value);
        }
        let tags: Vec<(String, String)> = generated.into_iter().collect();

        let removal_count = generator.below(6);
        let remove: Vec<String> = (0..removal_count).map(|_| generator.key()).collect();

        let expected: Vec<(String, String)> = tags
            .iter()
            .filter(|(key, _)| !remove.contains(key))
            .cloned()
            .collect();
        if expected.len() < tags.len() {
            saw_a_removal += 1;
        }
        if remove
            .iter()
            .any(|key| !tags.iter().any(|(stored, _)| stored == key))
        {
            saw_an_absent_key += 1;
        }
        if !expected.is_empty() {
            saw_a_kept_tag += 1;
        }

        let router = router();
        let name = format!("Generated Tags {case}");
        let arn = create_dashboard_with_tags(&router, &name, &tags).await;
        let request = json!({"resourceArn": arn, "resourceTagKeys": remove});

        ok_call(&router, "UntagResource", &request).await;
        let after = stored_tags(&router, &arn, &name).await;
        assert_eq!(
            after, expected,
            "case seed {SEED:#x} #{case} tags {tags:?} keys {remove:?}: expected {expected:?}, got {after:?}"
        );

        ok_call(&router, "UntagResource", &request).await;
        let after_repeat = stored_tags(&router, &arn, &name).await;
        assert_eq!(
            after_repeat, after,
            "case seed {SEED:#x} #{case} tags {tags:?} keys {remove:?}: a repeat must change nothing, got {after_repeat:?} after {after:?}"
        );
    }

    assert!(
        saw_a_removal > 0 && saw_an_absent_key > 0 && saw_a_kept_tag > 0,
        "case seed {SEED:#x}: the {CASES} generated cases must cover a real removal, an absent key and a kept tag, got {saw_a_removal} / {saw_an_absent_key} / {saw_a_kept_tag}"
    );
}
