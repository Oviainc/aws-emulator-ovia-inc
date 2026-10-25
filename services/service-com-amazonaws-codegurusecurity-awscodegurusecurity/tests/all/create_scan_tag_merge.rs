// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for what a tag map carried by a WRITE of a scan does to the tags the
//! scan already holds.
//!
//! RULE: a tagging input is additive — it writes the keys it names and leaves every other
//! key the resource carries in place; removal happens only through the operation modelled
//! for it (`UntagResource`). So `CreateScan` on an existing `scanName` (a new revision of
//! the same scan) upserts the keys its `tags` names and keeps the rest, exactly as
//! `TagResource` does, and the accumulated map stays inside the one bound every writer of
//! the tag store honours.
//!
//! The suite is a TABLE whose rows differ along what the rule generalises over: how many
//! keys the scan already carries and which call wrote them (`CreateScan` or
//! `TagResource`), how many keys the re-run names, whether those keys are new or already
//! held, whether the member is absent, empty or populated, and the key/value forms at the
//! published `TagMap` length bounds. Each case drives real HTTP through the production
//! front door (`MultiServiceRouter::dispatch`), so what is asserted is the wire behaviour
//! a `codeguru-security` SDK client sees.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_codegurusecurity_awscodegurusecurity::state::{
    STANDARD_SCAN_SETTLE_MILLIS, TAGS_PER_SCAN_QUOTA,
};

use crate::resource_tagging::{list_tags, tag, tag_object};
use crate::scan_lifecycle::{artifact_for, call, create_scan, harness, Answer};

/// A tag pair, spelled from `&str` for the tables below.
fn pair(key: &str, value: &str) -> (String, String) {
    (key.to_string(), value.to_string())
}

/// Wait past the clock-owed transition of a Standard scan run, so the next `CreateScan`
/// on the same scan name starts a revision instead of colliding with a run still
/// `InProgress`.
async fn wait_for_settle() {
    let span = u64::try_from(STANDARD_SCAN_SETTLE_MILLIS).unwrap_or(500);
    tokio::time::sleep(Duration::from_millis(span + 150)).await;
}

/// `CreateScan` for `scan_name`, carrying `tags` when the row names a map at all, on a
/// freshly minted artifact for that scan name — the whole client workflow for starting a
/// scan or a revision of one.
async fn run_scan(
    router: &MultiServiceRouter,
    scan_name: &str,
    tags: Option<&[(String, String)]>,
) -> Answer {
    let artifact = artifact_for(router, scan_name).await;
    let mut body = json!({ "scanName": scan_name });
    if let Some(tags) = tags {
        body["tags"] = tag_object(tags);
    }
    create_scan(router, body, &artifact).await
}

/// `GET /scans/{scanName}` — used to show whether a refused revision was started.
async fn get_scan(router: &MultiServiceRouter, scan_name: &str) -> Answer {
    call(router, "GET", &format!("/scans/{scan_name}"), None).await
}

/// The tags reported for `arn`, as a JSON object.
async fn reported_tags(router: &MultiServiceRouter, arn: &str) -> Value {
    let listed = list_tags(router, arn).await;
    assert_eq!(
        listed.status, 200,
        "ListTagsForResource({arn}): expected 200, got {listed:?}"
    );
    listed.get("tags").clone()
}

/// One row: the tags the scan is created with, the tags `TagResource` then adds, and the
/// `tags` member the re-running `CreateScan` carries (`None` = the member is absent).
#[derive(Debug)]
struct MergeCase {
    case: &'static str,
    created: Vec<(String, String)>,
    tagged: Vec<(String, String)>,
    rerun: Option<Vec<(String, String)>>,
}

impl MergeCase {
    /// The map the rule says the scan holds afterwards: every key any of the three calls
    /// wrote, each carrying the value the LAST call to name it gave it.
    fn expected(&self) -> Value {
        let mut merged: BTreeMap<String, String> = BTreeMap::new();
        for (key, value) in self
            .created
            .iter()
            .chain(self.tagged.iter())
            .chain(self.rerun.iter().flatten())
        {
            merged.insert(key.clone(), value.clone());
        }
        let pairs: Vec<(String, String)> = merged.into_iter().collect();
        tag_object(&pairs)
    }
}

/// The rows. Bounds are the published ones for `TagMap`
/// (`smithy_query(query="shape",shape="TagMap")`: length 0..200, keys `TagKey` len 1..128,
/// values `TagValue` len 0..256). The issue cited only the "one created key, one tagged
/// key, one new key on the re-run" row.
fn merge_cases() -> Vec<MergeCase> {
    vec![
        MergeCase {
            case: "the re-run carries no tags member at all",
            created: vec![pair("a", "1")],
            tagged: vec![pair("b", "2")],
            rerun: None,
        },
        MergeCase {
            case: "the re-run carries an empty tag map",
            created: vec![pair("a", "1")],
            tagged: vec![pair("b", "2")],
            rerun: Some(vec![]),
        },
        MergeCase {
            case: "the re-run names one key neither earlier call wrote",
            created: vec![pair("a", "1")],
            tagged: vec![pair("b", "2")],
            rerun: Some(vec![pair("c", "3")]),
        },
        MergeCase {
            case: "the re-run re-values the key TagResource added",
            created: vec![pair("a", "1")],
            tagged: vec![pair("b", "2")],
            rerun: Some(vec![pair("b", "rewritten")]),
        },
        MergeCase {
            case: "the re-run re-values the key the original CreateScan set",
            created: vec![pair("a", "1")],
            tagged: vec![pair("b", "2")],
            rerun: Some(vec![pair("a", "rewritten")]),
        },
        MergeCase {
            case: "the scan carried no tags until TagResource, the re-run adds one",
            created: vec![],
            tagged: vec![pair("team", "appsec")],
            rerun: Some(vec![pair("env", "prod")]),
        },
        MergeCase {
            case: "the scan was never tagged by TagResource",
            created: vec![pair("a", "1"), pair("b", "2")],
            tagged: vec![],
            rerun: Some(vec![pair("c", "3")]),
        },
        MergeCase {
            case: "the re-run names five keys, two of them already held",
            created: vec![pair("a", "1"), pair("b", "2")],
            tagged: vec![pair("c", "3")],
            rerun: Some(vec![
                pair("a", "one"),
                pair("c", "three"),
                pair("d", "4"),
                pair("e", "5"),
                pair("f", "6"),
            ]),
        },
        MergeCase {
            case: "keys and values at the published TagMap length bounds",
            created: vec![pair(&"k".repeat(128), "long-key"), pair("x", "")],
            tagged: vec![pair("y", &"v".repeat(256))],
            rerun: Some(vec![pair("z", ""), pair("q", &"w".repeat(256))]),
        },
        MergeCase {
            case: "a key carrying the punctuation a tag key allows, re-valued",
            created: vec![pair("ns:sub/part.v-1", "punctuated")],
            tagged: vec![pair("room", "code review 室 2")],
            rerun: Some(vec![pair("ns:sub/part.v-1", "re-punctuated")]),
        },
        MergeCase {
            case: "the re-run names nothing and the scan holds twenty keys",
            created: (0..10).map(|n| pair(&format!("c{n}"), "created")).collect(),
            tagged: (0..10).map(|n| pair(&format!("t{n}"), "tagged")).collect(),
            rerun: Some(vec![]),
        },
    ]
}

/// RULE: re-running a scan keeps every tag key the scan already carries that the request
/// does not name, and upserts the ones it does — a `CreateScan` is never a removal.
#[tokio::test]
async fn re_running_a_scan_merges_its_tags_instead_of_replacing_them() {
    let (_engine, router) = harness();

    for (index, row) in merge_cases().into_iter().enumerate() {
        let scan_name = format!("tag-merge-{index}");

        let created = run_scan(&router, &scan_name, Some(&row.created)).await;
        assert_eq!(
            created.status, 200,
            "{}: CreateScan expected 200, got {created:?}",
            row.case
        );
        let arn = created.str_of("scanNameArn");
        let first_run = created.str_of("runId");

        if !row.tagged.is_empty() {
            let tagged = tag(&router, &arn, tag_object(&row.tagged)).await;
            assert_eq!(
                tagged.status, 204,
                "{}: TagResource expected 204, got {tagged:?}",
                row.case
            );
        }

        let before = reported_tags(&router, &arn).await;
        let mut expected_before = row.created.clone();
        expected_before.extend(row.tagged.iter().cloned());
        let expected_before = {
            let mut merged: BTreeMap<String, String> = BTreeMap::new();
            for (key, value) in &expected_before {
                merged.insert(key.clone(), value.clone());
            }
            tag_object(&merged.into_iter().collect::<Vec<(String, String)>>())
        };
        assert_eq!(
            before, expected_before,
            "{}: before the re-run, expected {expected_before}, got {before}",
            row.case
        );

        wait_for_settle().await;
        let rerun = run_scan(&router, &scan_name, row.rerun.as_deref()).await;
        assert_eq!(
            rerun.status, 200,
            "{}: the re-running CreateScan expected 200, got {rerun:?}",
            row.case
        );
        assert_ne!(
            rerun.str_of("runId"),
            first_run,
            "{}: the re-run must start a new revision, got {rerun:?}",
            row.case
        );

        let after = reported_tags(&router, &arn).await;
        let expected = row.expected();
        assert_eq!(
            after, expected,
            "{}: after the re-run, expected the merged map {expected}, got {after}",
            row.case
        );

        // A read of the tags does not change what the next reader sees.
        let again = reported_tags(&router, &arn).await;
        assert_eq!(
            again, expected,
            "{}: a second read reports the same map, got {again}",
            row.case
        );
    }
}

/// RULE, the bound side: the accumulated tag map of one scan stays inside the single
/// bound every writer of the tag store honours, so a re-run whose merged map would pass
/// it is refused with the operation's modelled `ValidationException` (HTTP 400), the map
/// is left as it was AND no revision is started; a re-run that only re-values keys the
/// scan already holds adds nothing and is accepted at the bound.
#[tokio::test]
async fn a_re_run_cannot_push_a_scans_tag_map_past_its_bound() {
    let (_engine, router) = harness();
    let scan_name = "tag-merge-bound";

    // Fill the map to one short of the bound through the create, then take the last slot
    // with TagResource, so the scan sits exactly at the bound with keys from both writers.
    let created_tags: Vec<(String, String)> = (0..TAGS_PER_SCAN_QUOTA - 1)
        .map(|n| pair(&format!("key{n}"), &format!("value{n}")))
        .collect();
    let created = run_scan(&router, scan_name, Some(&created_tags)).await;
    assert_eq!(
        created.status, 200,
        "CreateScan at {} tags: expected 200, got {created:?}",
        created_tags.len()
    );
    let arn = created.str_of("scanNameArn");
    let filled = tag(&router, &arn, json!({ "last": "slot" })).await;
    assert_eq!(
        filled.status, 204,
        "TagResource taking the last slot: expected 204, got {filled:?}"
    );
    let at_bound = reported_tags(&router, &arn).await;
    assert_eq!(
        at_bound.as_object().map(serde_json::Map::len),
        Some(TAGS_PER_SCAN_QUOTA),
        "expected the scan to sit at the {TAGS_PER_SCAN_QUOTA}-tag bound, got {at_bound}"
    );

    let revisions_before = get_scan(&router, scan_name).await.get("numberOfRevisions").clone();

    // One new key on the re-run would leave the bound behind: refused, and nothing moved.
    wait_for_settle().await;
    let refused = run_scan(&router, scan_name, Some(&[pair("one-too-many", "v")])).await;
    assert_eq!(
        refused.status, 400,
        "one tag past the bound on a re-run: expected HTTP 400, got {refused:?}"
    );
    assert_eq!(
        refused.error_type, "ValidationException",
        "one tag past the bound on a re-run: expected error code ValidationException, got \
         {refused:?}"
    );
    let unchanged = reported_tags(&router, &arn).await;
    assert_eq!(
        unchanged, at_bound,
        "after the refusal: expected the tag map untouched, got {unchanged}"
    );
    let after_refusal = get_scan(&router, scan_name).await;
    assert_eq!(
        after_refusal.get("numberOfRevisions"),
        &revisions_before,
        "after the refusal: expected no revision to have been started, got {after_refusal:?}"
    );

    // Re-valuing keys the scan already holds adds no entry, so it is accepted AT the
    // bound — and still does not drop the keys it does not name.
    let accepted = run_scan(&router, scan_name, Some(&[pair("key0", "rewritten")])).await;
    assert_eq!(
        accepted.status, 200,
        "a re-run re-valuing a held key at the bound: expected 200, got {accepted:?}"
    );
    let after = reported_tags(&router, &arn).await;
    assert_eq!(
        after.as_object().map(serde_json::Map::len),
        Some(TAGS_PER_SCAN_QUOTA),
        "after the accepted re-run: expected {TAGS_PER_SCAN_QUOTA} tags, got {after}"
    );
    assert_eq!(
        after["key0"].as_str(),
        Some("rewritten"),
        "after the accepted re-run: expected the re-valued key to carry the new value, got {after}"
    );
    assert_eq!(
        after["last"].as_str(),
        Some("slot"),
        "after the accepted re-run: expected the key TagResource wrote to survive, got {after}"
    );
}
