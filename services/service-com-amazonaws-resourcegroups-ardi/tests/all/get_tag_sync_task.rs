// SPDX-License-Identifier: CC0-1.0

//! `GetTagSyncTask`, over the whole domain of what one stored tag-sync task can hold.
//!
//! Every request goes through the same `MultiServiceRouter::dispatch` the served
//! emulator uses (the harness is the `resource_group_lifecycle` suite's `Emulator`), so
//! these exercise the whole restJson1 path: routing, boundary validation, the handler,
//! and serialization.
//!
//! The rule under test: a read of a tag-sync task reports the record the store holds
//! for the task ARN the request addressed — every member verbatim, whatever its value,
//! size or encoding, selected from among every other task the store holds — and reports
//! nothing for a member the stored task does not carry. The sibling `tag_sync_tasks`
//! suite spans the two ways a task can sync and the identifier forms a task is started
//! with; this suite spans the VALUES a task's members take, at the bounds the model
//! publishes for them and one code point past, and pins that a read selects the
//! addressed task rather than any other stored one.
//!
//! Each rule is asserted over a TABLE of inputs rather than one row.

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use super::resource_group_lifecycle::{error_code, Emulator};
use super::tag_sync_tasks::{application_segment, group_arn, role_arn};

/// The published maximum length of the modelled `TagKey`, in code points.
///
/// Source: `smithy_query(query="operation",operation="GetTagSyncTask")` — `TagKey` is
/// `len 1..128, pattern ^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$`. A `@length` on a string
/// counts code points, so a key of this many multi-byte characters is admitted too.
const MAX_TAG_KEY_LENGTH: usize = 128;

/// The published maximum length of the modelled `TagValue`, in code points. Its
/// minimum is 0, so the empty value is admitted.
///
/// Source: `smithy_query(query="operation",operation="GetTagSyncTask")` — `TagValue` is
/// `len 0..256, pattern ^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$`.
const MAX_TAG_VALUE_LENGTH: usize = 256;

/// `n` code points of `unit` repeated.
fn repeated(unit: &str, n: usize) -> String {
    unit.chars().cycle().take(n).collect()
}

/// What a stored task syncs, and so what a read of it reports.
///
/// Owned, unlike the sibling suite's borrowed rows, because this table's values are
/// built to the published bounds rather than written out as literals.
#[derive(Debug, Clone)]
enum Sync {
    /// `TagKey` and `TagValue`: resources carrying the pair are synced.
    Pair { key: String, value: String },
    /// `ResourceQuery`: resources the query matches are synced.
    Query { query_type: &'static str, query: String },
}

impl Sync {
    /// The members a request sends, and a read reports, for this way of syncing.
    fn members(&self) -> Value {
        match self {
            Sync::Pair { key, value } => json!({ "TagKey": key, "TagValue": value }),
            Sync::Query { query_type, query } => {
                json!({ "ResourceQuery": { "Type": query_type, "Query": query } })
            }
        }
    }

    /// The members a task that syncs this way never reports.
    fn absent_members(&self) -> &'static [&'static str] {
        match self {
            Sync::Pair { .. } => &["ResourceQuery"],
            Sync::Query { .. } => &["TagKey", "TagValue"],
        }
    }
}

/// One task to store and read back. The rows differ in the size and encoding of what
/// the task syncs, in the length of the group's name, in the role given, and in the
/// region the group lives in — which is what the reporting rule generalises over.
#[derive(Debug, Clone)]
struct ReadCase {
    label: &'static str,
    region: &'static str,
    group: String,
    role: String,
    sync: Sync,
}

fn read_cases() -> Vec<ReadCase> {
    vec![
        ReadCase {
            label: "a tag key of the minimum length and the empty tag value",
            region: "us-east-1",
            group: "g".to_string(),
            role: role_arn("ResourceGroupsTagSync"),
            sync: Sync::Pair {
                key: "K".to_string(),
                value: String::new(),
            },
        },
        ReadCase {
            label: "a tag key and value at their published maximum lengths",
            region: "eu-west-1",
            group: "at-the-bounds".to_string(),
            role: role_arn("tag-sync/with-a-path"),
            sync: Sync::Pair {
                key: repeated("key", MAX_TAG_KEY_LENGTH),
                value: repeated("value", MAX_TAG_VALUE_LENGTH),
            },
        },
        ReadCase {
            label: "a tag pair of multi-byte code points at those maximum lengths",
            region: "ap-northeast-1",
            group: "multi-byte".to_string(),
            role: role_arn("ResourceGroupsTagSync"),
            sync: Sync::Pair {
                key: repeated("プロジェクト", MAX_TAG_KEY_LENGTH),
                value: repeated("équipe-日本", MAX_TAG_VALUE_LENGTH),
            },
        },
        ReadCase {
            label: "a tag pair using every punctuation the modelled pattern admits",
            region: "sa-east-1",
            group: "punctuation".to_string(),
            role: role_arn("Role_0+1=2,3.4@5-6"),
            sync: Sync::Pair {
                key: "a_b.c:d/e=f+g-h@i".to_string(),
                value: "a value_with.every:admitted/char=and+a-few@more 9".to_string(),
            },
        },
        ReadCase {
            label: "a resource query document of several hundred characters",
            region: "us-west-2",
            group: "a.long_query-group".to_string(),
            role: role_arn("ResourceGroupsTagSync"),
            sync: Sync::Query {
                query_type: "TAG_FILTERS_1_0",
                query: long_tag_filters_query(),
            },
        },
        ReadCase {
            label: "a group whose name is as long as a task ARN can carry",
            region: "eu-central-1",
            group: repeated("long-group-name.", 150),
            role: role_arn("ResourceGroupsTagSync"),
            sync: Sync::Pair {
                key: "Stage".to_string(),
                value: "Test".to_string(),
            },
        },
    ]
}

/// A tag-filters query document with many filters, so the stored query is long enough
/// that a reporting handler cannot be mistaken for one echoing a short literal.
fn long_tag_filters_query() -> String {
    let filters: Vec<Value> = (0..12)
        .map(|i| json!({ "Key": format!("Key{i}"), "Values": [format!("Value{i}"), "Shared"] }))
        .collect();
    json!({
        "ResourceTypeFilters": ["AWS::EC2::Instance", "AWS::S3::Bucket", "AWS::AllSupported"],
        "TagFilters": filters,
    })
    .to_string()
}

/// Seconds since the epoch, the units a restJson1 timestamp is written in.
fn epoch_seconds(at: SystemTime) -> f64 {
    at.duration_since(UNIX_EPOCH)
        .expect("a time after the epoch")
        .as_secs_f64()
}

/// Start `case`'s task on a group created for it, and return its task ARN together
/// with the `StartTagSyncTask` response.
async fn store_task(emulator: &Emulator, case: &ReadCase) -> (String, Value) {
    let label = case.label;
    let (status, created) = emulator
        .call_in(case.region, "/groups", &json!({ "Name": case.group }))
        .await;
    assert_eq!(
        status, 200,
        "case {label:?}: expected CreateGroup to answer 200, got {status} {created}",
    );

    let mut body = json!({ "Group": case.group, "RoleArn": case.role });
    let object = body.as_object_mut().expect("a JSON object");
    for (member, value) in case.sync.members().as_object().expect("a JSON object") {
        object.insert(member.clone(), value.clone());
    }
    let (status, started) = emulator
        .call_in(case.region, "/start-tag-sync-task", &body)
        .await;
    assert_eq!(
        status, 200,
        "case {label:?}: expected StartTagSyncTask to answer 200, got {status} {started}",
    );
    let task_arn = started["TaskArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {label:?}: expected a TaskArn, got {started}"))
        .to_string();
    (task_arn, started)
}

/// Every stored task is reported back from its own record, whatever its members hold,
/// and is selected by the task ARN the read addressed rather than by anything else the
/// store holds: the whole table is stored FIRST and only then read, so a handler
/// reporting any other task's record fails here.
#[tokio::test]
async fn every_stored_task_is_reported_from_its_own_record() {
    let emulator = Emulator::new();
    let before = epoch_seconds(SystemTime::now());
    let mut stored: Vec<(ReadCase, String)> = Vec::new();
    for case in read_cases() {
        let (task_arn, _) = store_task(&emulator, &case).await;
        assert!(
            stored.iter().all(|(_, seen)| seen != &task_arn),
            "case {:?}: expected a task ARN of its own, got {task_arn:?} again among {:?}",
            case.label,
            stored.iter().map(|(_, seen)| seen).collect::<Vec<_>>(),
        );
        stored.push((case, task_arn));
    }

    for (case, task_arn) in &stored {
        let label = case.label;
        let arn = group_arn(case.region, &case.group);
        let (status, task) = emulator
            .call_in(
                case.region,
                "/get-tag-sync-task",
                &json!({ "TaskArn": task_arn }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected GetTagSyncTask to answer 200, got {status} {task}",
        );
        assert_eq!(
            (
                task["GroupArn"].as_str(),
                task["GroupName"].as_str(),
                task["TaskArn"].as_str(),
                task["RoleArn"].as_str(),
                task["Status"].as_str(),
            ),
            (
                Some(arn.as_str()),
                Some(case.group.as_str()),
                Some(task_arn.as_str()),
                Some(case.role.as_str()),
                Some("ACTIVE"),
            ),
            "case {label:?}: expected the task stored for {task_arn} reported back as \
             ACTIVE, got {task}",
        );
        for (member, value) in case.sync.members().as_object().expect("a JSON object") {
            assert_eq!(
                &task[member], value,
                "case {label:?}: expected {member} reported verbatim as {value}, got {task}",
            );
        }
        for absent in case.sync.absent_members() {
            assert!(
                task.get(absent).is_none(),
                "case {label:?}: expected no {absent} on a task that does not sync one, \
                 got {task}",
            );
        }
        assert!(
            task.get("ErrorMessage").is_none(),
            "case {label:?}: expected no ErrorMessage on a task that did not fail, got {task}",
        );
        // The task was created by the call that started it, so its timestamp falls
        // inside the window this test ran in rather than being a fixed value.
        let after = epoch_seconds(SystemTime::now());
        let created_at = task["CreatedAt"]
            .as_f64()
            .unwrap_or_else(|| panic!("case {label:?}: expected a CreatedAt, got {task}"));
        assert!(
            (before..=after).contains(&created_at),
            "case {label:?}: expected CreatedAt within [{before}, {after}], got {created_at}",
        );
        assert!(
            application_segment(task_arn, &arn).is_some(),
            "case {label:?}: expected the reported task ARN nested under {arn}, \
             got {task_arn:?}",
        );

        // Reading reports, it does not write: the same read answers the same record,
        // after every other task of the table has been read too.
        let again = emulator
            .call_in(
                case.region,
                "/get-tag-sync-task",
                &json!({ "TaskArn": task_arn }),
            )
            .await;
        assert_eq!(
            (again.0, &again.1),
            (200, &task),
            "case {label:?}: expected a repeated read to answer the same record, got {} {}",
            again.0,
            again.1,
        );
    }
}

/// The table covers both the minimum and the published maximum of each bounded member
/// it sends, so it spans those ranges rather than one point inside them.
#[test]
fn the_table_spans_the_published_bounds_of_a_tag_pair() {
    let pairs: Vec<(usize, usize)> = read_cases()
        .iter()
        .filter_map(|case| match &case.sync {
            Sync::Pair { key, value } => Some((key.chars().count(), value.chars().count())),
            Sync::Query { .. } => None,
        })
        .collect();
    for (member, bound, lengths) in [
        ("TagKey", 1, pairs.iter().map(|(key, _)| *key).collect::<Vec<_>>()),
        ("TagKey", MAX_TAG_KEY_LENGTH, pairs.iter().map(|(key, _)| *key).collect()),
        ("TagValue", 0, pairs.iter().map(|(_, value)| *value).collect()),
        (
            "TagValue",
            MAX_TAG_VALUE_LENGTH,
            pairs.iter().map(|(_, value)| *value).collect(),
        ),
    ] {
        assert!(
            lengths.contains(&bound),
            "case \"{member} of {bound} code points\": expected a row at that bound, \
             got {lengths:?}",
        );
    }
}

/// A tag pair one code point past a published bound never reaches the handler, so no
/// task is stored for it and nothing is readable; the bound itself still stores a task
/// that reads back.
#[tokio::test]
async fn a_tag_pair_past_a_published_bound_stores_no_readable_task() {
    let emulator = Emulator::new();
    emulator.ok("/groups", &json!({ "Name": "bounds" })).await;
    let role = role_arn("ResourceGroupsTagSync");
    let refused = [
        (
            "a tag key one code point past its maximum",
            repeated("key", MAX_TAG_KEY_LENGTH + 1),
            "Test".to_string(),
        ),
        (
            "a multi-byte tag key one code point past its maximum",
            repeated("プロジェクト", MAX_TAG_KEY_LENGTH + 1),
            "Test".to_string(),
        ),
        (
            "a tag value one code point past its maximum",
            "Stage".to_string(),
            repeated("value", MAX_TAG_VALUE_LENGTH + 1),
        ),
        (
            "a multi-byte tag value one code point past its maximum",
            "Stage".to_string(),
            repeated("équipe-日本", MAX_TAG_VALUE_LENGTH + 1),
        ),
        (
            "a tag key one code point below its minimum",
            String::new(),
            "Test".to_string(),
        ),
    ];
    for (label, key, value) in refused {
        let (status, answered) = emulator
            .call(
                "/start-tag-sync-task",
                &json!({ "Group": "bounds", "RoleArn": role, "TagKey": key, "TagValue": value }),
            )
            .await;
        assert_eq!(
            (status, error_code(&answered)),
            (400, "BadRequestException"),
            "case {label:?} (key of {} code points, value of {}): expected 400 \
             BadRequestException, got {status} {answered}",
            key.chars().count(),
            value.chars().count(),
        );
        assert!(
            answered.get("TaskArn").is_none(),
            "case {label:?}: expected a refusal to mint no task ARN, got {answered}",
        );
    }

    // The bound itself is admitted, and the task it stored is the one a read reports.
    let key = repeated("key", MAX_TAG_KEY_LENGTH);
    let value = repeated("value", MAX_TAG_VALUE_LENGTH);
    let started = emulator
        .ok(
            "/start-tag-sync-task",
            &json!({ "Group": "bounds", "RoleArn": role, "TagKey": key, "TagValue": value }),
        )
        .await;
    let task_arn = started["TaskArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case \"at the bound\": expected a TaskArn, got {started}"))
        .to_string();
    let task = emulator
        .ok("/get-tag-sync-task", &json!({ "TaskArn": &task_arn }))
        .await;
    assert_eq!(
        (
            task["TagKey"].as_str(),
            task["TagValue"].as_str(),
            task["TaskArn"].as_str(),
        ),
        (Some(key.as_str()), Some(value.as_str()), Some(task_arn.as_str())),
        "case \"at the bound\": expected the stored pair reported back verbatim, got {task}",
    );
}
