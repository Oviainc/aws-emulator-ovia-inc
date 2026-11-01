// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for what a tag write does to the deployment RECORD it writes.
//!
//! The rule under test is about the record rather than about one operation:
//! `modifiedAt` is the instant the deployment record last CHANGED, and the tag set is
//! part of that record (`GetDeployment` reports both in one response), so a call that
//! changes the set — `TagResource` or `UntagResource` — refreshes the timestamp just as
//! the settings/status writers (`UpdateDeployment`, `DeleteDeployment`, the clock-owed
//! status steps) do, and a call that changes nothing leaves the whole record, timestamp
//! included, as it was.
//!
//! Every assertion runs over a TABLE whose rows differ along what the rule generalises
//! over: the operation making the write (tag / untag), how many keys it names, whether
//! each named key is new, already held at that value, already held at another value or
//! absent, and the value's own shape (including the empty value `TagValue` admits).
//! Each row also checks the two readers of the field — `GetDeployment`'s
//! `DeploymentData` and `ListDeployments`' `DeploymentDataSummary` — agree, so the rule
//! is about the stored record and not about one projection of it.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::{json, Value};

use crate::deployment_lifecycle::get;
use crate::deployment_update::{conforming_specifications, pattern_data, update_body};
use crate::harness::Harness;
use crate::resource_tagging::{
    deployment_arn, deployment_to_tag, deployment_tags, tags_path, untag_path,
};

/// Long enough for every clock-owed step a CREATE schedules to come due (the three
/// `state::TRANSITION_SPAN`s a create walks, plus slack). The emulator never sleeps;
/// this is the test waiting for the clock.
///
/// A suite about `modifiedAt` waits this out before the table: a step the clock still
/// owes is itself a change to the record, so a tagging call made while one is pending
/// would move the timestamp whatever the tag write did.
const CREATE_SETTLE: Duration = Duration::from_millis(2_100);

/// Long enough for the clock-owed step an `UpdateDeployment` schedules to come due (one
/// `state::TRANSITION_SPAN` plus slack). See [`CREATE_SETTLE`].
const UPDATE_SETTLE: Duration = Duration::from_millis(1_200);

/// The workload/pattern pair the suite's deployments are created against — the same one
/// `resource_tagging`'s `deployment_to_tag` uses.
const WORKLOAD: &str = "SAP";
/// See [`WORKLOAD`].
const PATTERN: &str = "SapHanaSingle";

/// The tags a created deployment carries (`deployment_lifecycle::create_body`'s
/// `tags`), which every row below starts from.
fn created_with() -> BTreeMap<String, String> {
    BTreeMap::from([
        ("Owner".to_string(), "emulator-tests".to_string()),
        ("Workload".to_string(), "SAP".to_string()),
    ])
}

/// The write one row makes.
#[derive(Debug, Clone)]
enum Write {
    /// `TagResource` with these pairs.
    Tag(Vec<(&'static str, &'static str)>),
    /// `UntagResource` with these keys.
    Untag(Vec<&'static str>),
}

/// One row of the table: a write, and whether it leaves the record's tag set different
/// from the one it found.
#[derive(Debug, Clone)]
struct Row {
    case: &'static str,
    write: Write,
    changes_the_record: bool,
}

/// The rows, applied in order to ONE deployment, so each row's "already held" /
/// "absent" state is the one the rows before it produced.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a new key",
            write: Write::Tag(vec![("Team", "platform")]),
            changes_the_record: true,
        },
        Row {
            case: "the same key with the same value again",
            write: Write::Tag(vec![("Team", "platform")]),
            changes_the_record: false,
        },
        Row {
            case: "a held key taking a different value",
            write: Write::Tag(vec![("Team", "infrastructure")]),
            changes_the_record: true,
        },
        Row {
            case: "a key the create seeded, at the value it seeded",
            write: Write::Tag(vec![("Owner", "emulator-tests")]),
            changes_the_record: false,
        },
        Row {
            case: "two new keys at once",
            write: Write::Tag(vec![("Env", "prod"), ("Tier", "db")]),
            changes_the_record: true,
        },
        Row {
            case: "a new key beside an unchanged one",
            write: Write::Tag(vec![("Zone", "eu-west-1a"), ("Env", "prod")]),
            changes_the_record: true,
        },
        Row {
            case: "every named key already held at its value",
            write: Write::Tag(vec![("Env", "prod"), ("Tier", "db"), ("Zone", "eu-west-1a")]),
            changes_the_record: false,
        },
        Row {
            // `TagValue` is `@length(min: 0, ..)`, so the empty value is a value.
            case: "a new key with the empty value",
            write: Write::Tag(vec![("Empty", "")]),
            changes_the_record: true,
        },
        Row {
            case: "the empty value written again",
            write: Write::Tag(vec![("Empty", "")]),
            changes_the_record: false,
        },
        Row {
            case: "the empty value replaced by a non-empty one",
            write: Write::Tag(vec![("Empty", "filled")]),
            changes_the_record: true,
        },
        Row {
            case: "untagging a held key",
            write: Write::Untag(vec!["Team"]),
            changes_the_record: true,
        },
        Row {
            case: "untagging that key again",
            write: Write::Untag(vec!["Team"]),
            changes_the_record: false,
        },
        Row {
            case: "untagging a key the resource never carried",
            write: Write::Untag(vec!["NeverWritten"]),
            changes_the_record: false,
        },
        Row {
            case: "untagging two held keys at once",
            write: Write::Untag(vec!["Env", "Tier"]),
            changes_the_record: true,
        },
        Row {
            case: "untagging a held key beside an absent one",
            write: Write::Untag(vec!["Zone", "NeverWritten"]),
            changes_the_record: true,
        },
        Row {
            case: "untagging only absent keys",
            write: Write::Untag(vec!["NeverWritten", "AlsoAbsent"]),
            changes_the_record: false,
        },
    ]
}

/// Apply `write` to `arn`, requiring the 200 a successful tagging call answers with.
async fn apply(harness: &Harness, case: &str, arn: &str, write: &Write) {
    match write {
        Write::Tag(pairs) => {
            let tags: BTreeMap<&str, &str> = pairs.iter().copied().collect();
            harness
                .expect_ok(
                    case,
                    "POST",
                    &tags_path(arn),
                    Some(&json!({"tags": tags})),
                )
                .await;
        }
        Write::Untag(keys) => {
            let keys: Vec<String> = keys.iter().map(|key| (*key).to_string()).collect();
            harness
                .expect_ok(case, "DELETE", &untag_path(arn, &keys), None)
                .await;
        }
    }
}

/// `expected` with `write` applied, the way the service's merge/remove semantics do.
fn apply_expected(expected: &mut BTreeMap<String, String>, write: &Write) {
    match write {
        Write::Tag(pairs) => {
            for (key, value) in pairs {
                expected.insert((*key).to_string(), (*value).to_string());
            }
        }
        Write::Untag(keys) => {
            for key in keys {
                expected.remove(*key);
            }
        }
    }
}

/// A timestamp member of a reported deployment, as the epoch seconds the wire carries.
fn stamp(case: &str, deployment: &Value, member: &str) -> f64 {
    deployment[member].as_f64().unwrap_or_else(|| {
        panic!("case {case:?}: expected a numeric {member}, got {}", deployment[member])
    })
}

/// `createdAt` and `modifiedAt` as `GetDeployment` reports them, checking the second
/// reader of the same record (`ListDeployments`' summary) reports the same
/// `modifiedAt`.
async fn timestamps(harness: &Harness, case: &str, id: &str) -> (f64, f64) {
    let deployment = get(harness, case, id).await;
    let created = stamp(case, &deployment, "createdAt");
    let modified = stamp(case, &deployment, "modifiedAt");
    let listed = harness.ok(case, "/listDeployments", json!({})).await;
    let summary = listed["deployments"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: ListDeployments reported no deployments: {listed}"))
        .iter()
        .find(|summary| summary["id"].as_str() == Some(id))
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: ListDeployments did not report {id}: {listed}"));
    assert_eq!(
        stamp(case, &summary, "modifiedAt"),
        modified,
        "case {case:?}: ListDeployments must report the record's modifiedAt, got {}",
        summary["modifiedAt"]
    );
    assert_eq!(
        stamp(case, &summary, "createdAt"),
        created,
        "case {case:?}: ListDeployments must report the record's createdAt, got {}",
        summary["createdAt"]
    );
    (created, modified)
}

#[tokio::test]
async fn a_tag_write_that_changes_the_record_refreshes_modified_at() {
    let harness = Harness::new();
    let id = deployment_to_tag(&harness, "modified-at-on-tagging").await;
    let arn = deployment_arn(&id);
    // The clock owes this record nothing from here on, so every move of `modifiedAt`
    // below is the tagging call's and not a status step's.
    std::thread::sleep(CREATE_SETTLE);

    let mut expected = created_with();
    let (born, mut previous) = timestamps(&harness, "the settled create", &id).await;
    assert!(
        previous >= born,
        "the settled create must report modifiedAt {previous} at or after createdAt {born}"
    );
    assert_eq!(
        deployment_tags(&harness, "the settled create", &id).await,
        expected,
        "the deployment must start from the tags it was created with"
    );

    for Row {
        case,
        write,
        changes_the_record,
    } in rows()
    {
        apply(&harness, case, &arn, &write).await;
        apply_expected(&mut expected, &write);

        let (created, modified) = timestamps(&harness, case, &id).await;
        assert_eq!(
            deployment_tags(&harness, case, &id).await,
            expected,
            "case {case:?}: the write must leave the tag set it names"
        );
        assert_eq!(
            created, born,
            "case {case:?}: createdAt must not move, expected {born}, got {created}"
        );
        if changes_the_record {
            assert!(
                modified > previous,
                "case {case:?}: a tag write that changes the record must refresh \
                 modifiedAt past {previous}, got {modified}"
            );
        } else {
            assert_eq!(
                modified, previous,
                "case {case:?}: a tag write that changes nothing must leave modifiedAt \
                 at {previous}, got {modified}"
            );
        }
        previous = modified;
    }
}

#[tokio::test]
async fn a_refused_tagging_call_leaves_modified_at_where_it_was() {
    let harness = Harness::new();
    let id = deployment_to_tag(&harness, "modified-at-on-refusals").await;
    let arn = deployment_arn(&id);
    std::thread::sleep(CREATE_SETTLE);
    let (_, before) = timestamps(&harness, "before the refusals", &id).await;
    let tags_before = deployment_tags(&harness, "before the refusals", &id).await;

    // A refusal writes nothing, so it is not a modification of the record — one row per
    // class of refusal the tagging operations answer, against the resource that exists.
    // The bounds come from `smithy_query(query="shape", shape="Tags,TagKey")`: `Tags` is
    // `@length(min: 1, max: 200)` and `TagKey` is `@length(min: 1, max: 128)` with
    // `@pattern("^[a-zA-Z0-9 _.:/=+@-]+$")`.
    let long_key = "k".repeat(129);
    harness
        .expect_refused(
            "TagResource with no tags at all",
            "POST",
            &tags_path(&arn),
            Some(&json!({"tags": {}})),
            400,
            "ValidationException",
        )
        .await;
    harness
        .expect_refused(
            "TagResource with a key past its length bound",
            "POST",
            &tags_path(&arn),
            Some(&json!({"tags": {long_key.clone(): "v"}})),
            400,
            "ValidationException",
        )
        .await;
    harness
        .expect_refused(
            "UntagResource with a key outside the key pattern",
            "DELETE",
            &untag_path(&arn, &["no\u{a3}good".to_string()]),
            None,
            400,
            "ValidationException",
        )
        .await;
    harness
        .expect_refused(
            "TagResource on an ARN that cannot name a deployment",
            "POST",
            &tags_path("arn:aws:launchwizard:eu-west-1:000000000000:workload/SAP"),
            Some(&json!({"tags": {"Owner": "someone-else"}})),
            400,
            "BadRequestException",
        )
        .await;

    let (_, after) = timestamps(&harness, "after the refusals", &id).await;
    assert_eq!(
        after, before,
        "a refused tagging call must leave modifiedAt at {before}, got {after}"
    );
    assert_eq!(
        deployment_tags(&harness, "after the refusals", &id).await,
        tags_before,
        "a refused tagging call must leave the tag set alone"
    );
}

#[tokio::test]
async fn every_writer_of_the_record_refreshes_modified_at() {
    let harness = Harness::new();
    let id = deployment_to_tag(&harness, "modified-at-across-writers").await;
    let arn = deployment_arn(&id);
    std::thread::sleep(CREATE_SETTLE);
    let (_, mut previous) = timestamps(&harness, "the settled create", &id).await;

    let data = pattern_data(&harness, "the pattern's schema", WORKLOAD, PATTERN).await;
    let specifications = conforming_specifications("the pattern's schema", &data, false);

    // One row per operation that writes this record: the two tagging writers and the
    // settings writer they are being held to the same rule as. Each one must leave
    // `modifiedAt` later than the write before it.
    let writers: Vec<(&str, Write)> = vec![
        ("TagResource", Write::Tag(vec![("Stage", "one")])),
        ("UntagResource", Write::Untag(vec!["Stage"])),
    ];
    for (case, write) in &writers {
        apply(&harness, case, &arn, write).await;
        let (_, modified) = timestamps(&harness, case, &id).await;
        assert!(
            modified > previous,
            "case {case:?}: a write of the record must refresh modifiedAt past \
             {previous}, got {modified}"
        );
        previous = modified;
    }

    harness
        .ok(
            "UpdateDeployment",
            "/updateDeployment",
            update_body(&id, &specifications),
        )
        .await;
    let (_, updated) = timestamps(&harness, "UpdateDeployment", &id).await;
    assert!(
        updated > previous,
        "case \"UpdateDeployment\": a write of the record must refresh modifiedAt past \
         {previous}, got {updated}"
    );

    // And the clock-owed step the update schedules is itself a change to the record, so
    // it refreshes the field too — at its own due instant, not at the time it was read.
    std::thread::sleep(UPDATE_SETTLE);
    let (_, settled) = timestamps(&harness, "the settled update", &id).await;
    assert!(
        settled > updated,
        "case \"the settled update\": the clock-owed step must refresh modifiedAt past \
         {updated}, got {settled}"
    );
    let twice = timestamps(&harness, "a second read of the settled update", &id).await;
    assert_eq!(
        twice.1, settled,
        "reading the record twice must not move modifiedAt, expected {settled}, got {}",
        twice.1
    );
}
