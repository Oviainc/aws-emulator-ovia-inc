// SPDX-License-Identifier: CC0-1.0

//! The tag rules on the CREATE path, end to end over HTTP.
//!
//! The rule under test: every operation that takes this service's `TagMap` holds it to
//! the same AWS-wide tagging rules, so a request whose tag map carries an empty key, a
//! key beginning with the reserved `aws:` prefix, a key or value past the published
//! length ceiling, or more tags than a resource may carry is refused HTTP 400
//! `ValidationException` and leaves nothing behind — whether the map arrived on
//! `CreateTask` or on `TagResource`. The mirror of it is just as much part of the rule:
//! a map the rules accept is persisted whole and reported by every later reader of the
//! record.
//!
//! Each case is a TABLE whose rows differ along what the rule generalises over — the tag
//! key, the tag value, their lengths at and past each published bound, the tag count at
//! and past the quota, and which operation carried the map — not the one recorded
//! request. Every row goes through the router and is read back through `DescribeTask` and
//! `ListTagsForResource`, so an answer that is not computed from persisted state cannot
//! pass.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::device_task_lifecycle::{
    assert_not_found, assert_validation, call, create_task, describe_task, emulator, ids_of,
    list_tags, ok, string, tag_resource, whole_inventory,
};

/// The AWS-wide tagging quotas every operation taking a `TagMap` enforces: 50 tags per
/// resource, 128 Unicode characters per key, 256 per value ("Maximum number of tags per
/// resource: 50", "Maximum key length: 128", "Maximum value length: 256" — the service's
/// model constrains `TagMap` with none of them). Spelled out here rather than imported,
/// so a wrong bound in the implementation cannot make this suite pass.
const MAX_TAGS: usize = 50;
const MAX_TAG_KEY_LENGTH: usize = 128;
const MAX_TAG_VALUE_LENGTH: usize = 256;

/// The tag-key prefix AWS reserves for itself, which no request may introduce.
const RESERVED_PREFIX: &str = "aws:";

/// One row of the tag table: a map, and whether the rules accept it.
struct Row {
    case: &'static str,
    tags: Value,
    accepted: bool,
}

/// Every row of the tag table. Built rather than `const`, because the length and count
/// rows are generated from the published bounds above.
fn rows() -> Vec<Row> {
    let long_key = "k".repeat(MAX_TAG_KEY_LENGTH);
    let too_long_key = "k".repeat(MAX_TAG_KEY_LENGTH + 1);
    let long_value = "v".repeat(MAX_TAG_VALUE_LENGTH);
    let too_long_value = "v".repeat(MAX_TAG_VALUE_LENGTH + 1);
    let full_map: Value = (0..MAX_TAGS)
        .map(|index| (format!("key-{index:02}"), json!("v")))
        .collect::<serde_json::Map<String, Value>>()
        .into();
    let over_full_map: Value = (0..=MAX_TAGS)
        .map(|index| (format!("key-{index:02}"), json!("v")))
        .collect::<serde_json::Map<String, Value>>()
        .into();

    vec![
        Row {
            case: "no tags at all",
            tags: json!({}),
            accepted: true,
        },
        Row {
            case: "one ordinary tag",
            tags: json!({ "Name": "nightly-reboot" }),
            accepted: true,
        },
        Row {
            case: "several ordinary tags",
            tags: json!({ "Env": "prod", "Name": "nightly", "Owner": "ops" }),
            accepted: true,
        },
        Row {
            case: "an empty value",
            tags: json!({ "Name": "" }),
            accepted: true,
        },
        Row {
            case: "a non-ASCII key and value",
            tags: json!({ "propriétaire": "équipe" }),
            accepted: true,
        },
        // A key the prefix rule must NOT catch: the reserved prefix includes its colon,
        // and the rule is a prefix test, not a substring one.
        Row {
            case: "a key that only looks reserved",
            tags: json!({ "awsome": "yes", "AWSNOTCOLON": "yes", "team:aws:owner": "yes" }),
            accepted: true,
        },
        Row {
            case: "an empty key",
            tags: json!({ "": "v" }),
            accepted: false,
        },
        Row {
            case: "the reserved prefix",
            tags: json!({ "aws:created-by": "test" }),
            accepted: false,
        },
        // Tag keys are case-insensitive for the reserved prefix: `AWS:` is reserved too.
        Row {
            case: "the reserved prefix, upper case",
            tags: json!({ "AWS:cost-center": "42" }),
            accepted: false,
        },
        Row {
            case: "the reserved prefix beside an acceptable tag",
            tags: json!({ "Name": "nightly", "aws:cost-center": "42" }),
            accepted: false,
        },
        Row {
            case: "a key at the length ceiling",
            tags: json!({ long_key.clone(): "v" }),
            accepted: true,
        },
        Row {
            case: "a key one past the length ceiling",
            tags: json!({ too_long_key.clone(): "v" }),
            accepted: false,
        },
        Row {
            case: "a value at the length ceiling",
            tags: json!({ "Name": long_value.clone() }),
            accepted: true,
        },
        Row {
            case: "a value one past the length ceiling",
            tags: json!({ "Name": too_long_value.clone() }),
            accepted: false,
        },
        Row {
            case: "exactly as many tags as a resource may carry",
            tags: full_map,
            accepted: true,
        },
        Row {
            case: "one tag more than a resource may carry",
            tags: over_full_map,
            accepted: false,
        },
    ]
}

/// A row's tag map as a comparable map.
fn expected_map(tags: &Value) -> BTreeMap<String, String> {
    tags.as_object()
        .expect("every row's tags are a JSON object")
        .iter()
        .map(|(key, value)| {
            (
                key.clone(),
                value
                    .as_str()
                    .expect("every row's tag values are strings")
                    .to_string(),
            )
        })
        .collect()
}

/// The `tags` member of a report, as a comparable map. Required to be present: a record
/// carrying no tags reports an EMPTY map, which is not the member being absent.
fn tags_of(body: &Value, case: &str) -> BTreeMap<String, String> {
    let tags = body
        .get("tags")
        .unwrap_or_else(|| panic!("case {case:?}: expected a tags member, got {body}"));
    let object = tags
        .as_object()
        .unwrap_or_else(|| panic!("case {case:?}: expected tags to be a map, got {body}"));
    object
        .iter()
        .map(|(key, value)| {
            (
                key.clone(),
                value
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: tag {key:?} must be a string, got {value}")
                    })
                    .to_string(),
            )
        })
        .collect()
}

/// The `tasks` this account holds, as `ListTasks` reports them.
async fn listed_tasks(router: &MultiServiceRouter, case: &str) -> Vec<Value> {
    let body = ok(&call(router, "GET", "/tasks", None).await, case);
    body["tasks"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a tasks array, got {body}"))
        .clone()
}

/// One activated device id to aim a task at.
async fn a_device(router: &MultiServiceRouter, case: &str) -> String {
    let devices = whole_inventory(router, case).await;
    ids_of(&devices, case)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("case {case:?}: the emulator must activate a device"))
}

#[tokio::test]
async fn create_task_refuses_a_tag_map_the_tagging_rules_reject_and_mints_nothing() {
    for row in rows() {
        // A row runs against its own emulator, so one row's accepted tags cannot change
        // the next row's answer.
        let router = emulator();
        let device = a_device(&router, row.case).await;
        let answer = create_task(
            &router,
            json!({
                "targets": [device],
                "command": { "reboot": {} },
                "tags": row.tags.clone(),
            }),
        )
        .await;

        if !row.accepted {
            assert_validation(&answer, row.case);
            // Refused means nothing was minted: no task to describe, no task to list.
            assert_eq!(
                listed_tasks(&router, row.case).await,
                Vec::<Value>::new(),
                "case {:?}: a refused CreateTask must mint no task, got {}",
                row.case,
                answer.body
            );
            continue;
        }

        let body = ok(&answer, row.case);
        let task_id = string(&body, "taskId", row.case);
        let task_arn = string(&body, "taskArn", row.case);
        let expected = expected_map(&row.tags);

        // Read the accepted map back through both readers of the record.
        let described = ok(&describe_task(&router, &task_id).await, row.case);
        assert_eq!(
            tags_of(&described, row.case),
            expected,
            "case {:?}: DescribeTask must report exactly the tags CreateTask persisted",
            row.case
        );
        let read = ok(&list_tags(&router, &task_arn).await, row.case);
        assert_eq!(
            tags_of(&read, row.case),
            expected,
            "case {:?}: ListTagsForResource must report exactly the tags CreateTask persisted",
            row.case
        );
        let listed = listed_tasks(&router, row.case).await;
        assert_eq!(
            listed.len(),
            1,
            "case {:?}: an accepted CreateTask must mint exactly one task, got {listed:?}",
            row.case
        );
    }
}

#[tokio::test]
async fn the_same_tag_map_gets_the_same_verdict_on_create_and_on_tag_resource() {
    // The rule is one rule, not one per operation: for every row, the create path and the
    // tagging path agree on whether the map is acceptable, and a refusal on either path
    // is the same HTTP 400 `ValidationException`.
    for row in rows() {
        let router = emulator();
        let device = a_device(&router, row.case).await;

        let created = create_task(
            &router,
            json!({
                "targets": [&device],
                "command": { "reboot": {} },
                "tags": row.tags.clone(),
            }),
        )
        .await;

        // A task with no tags of its own, so the tagging path sees the same empty record
        // the create path writes into.
        let plain = ok(
            &create_task(
                &router,
                json!({ "targets": [&device], "command": { "unlock": {} } }),
            )
            .await,
            row.case,
        );
        let plain_arn = string(&plain, "taskArn", row.case);
        let tagged = tag_resource(&router, &plain_arn, row.tags.clone()).await;

        assert_eq!(
            created.status, tagged.status,
            "case {:?}: CreateTask answered {} and TagResource {} for the same tag map \
             ({} / {})",
            row.case, created.status, tagged.status, created.body, tagged.body
        );
        if row.accepted {
            ok(&created, row.case);
            ok(&tagged, row.case);
        } else {
            assert_validation(&created, row.case);
            assert_validation(&tagged, row.case);
            // And neither refusal wrote anything: the task the tagging path addressed
            // still carries no tags.
            let described = ok(&describe_task(&router, &string(&plain, "taskId", row.case)).await, row.case);
            assert_eq!(
                tags_of(&described, row.case),
                BTreeMap::new(),
                "case {:?}: a refused tag write must leave the task's tags alone",
                row.case
            );
        }
    }
}

#[tokio::test]
async fn create_task_checks_its_tag_map_before_it_resolves_its_targets() {
    // The tag map is a property of the REQUEST, so it is judged before the request's
    // targets are looked up: a create carrying both an unacceptable tag and a target
    // naming no managed device is answered as the validation failure it is, and still
    // mints nothing.
    let router = emulator();
    let case = "a reserved key and an unknown target";
    let reserved_key = format!("{RESERVED_PREFIX}created-by");
    let answer = create_task(
        &router,
        json!({
            "targets": ["smd-ffffffffffffffff"],
            "command": { "reboot": {} },
            "tags": { reserved_key.clone(): "test" },
        }),
    )
    .await;
    assert_validation(&answer, case);
    assert_eq!(
        listed_tasks(&router, case).await,
        Vec::<Value>::new(),
        "case {case:?}: a refused CreateTask must mint no task"
    );

    // The same request with an acceptable tag map is answered by the target rule
    // instead — so the ordering above is what refused it, not the target.
    let answer = create_task(
        &router,
        json!({
            "targets": ["smd-ffffffffffffffff"],
            "command": { "reboot": {} },
            "tags": { "Name": "nightly" },
        }),
    )
    .await;
    assert_not_found(&answer, "an unknown target alone");
}

#[tokio::test]
async fn an_idempotent_create_is_refused_for_an_unacceptable_tag_map_too() {
    // The same ordering against the `@idempotencyToken` replay: a token that already
    // names a task, resent with a tag map the rules reject, is the validation failure —
    // an unacceptable request is not made acceptable by having been sent before — while
    // the same token with an acceptable map still reports the task it first minted.
    let router = emulator();
    let case = "an idempotent create";
    let device = a_device(&router, case).await;
    let reserved_key = format!("{RESERVED_PREFIX}created-by");
    let first = ok(
        &create_task(
            &router,
            json!({
                "targets": [&device],
                "command": { "reboot": {} },
                "clientToken": "one-token",
            }),
        )
        .await,
        case,
    );

    let refused = create_task(
        &router,
        json!({
            "targets": [&device],
            "command": { "reboot": {} },
            "clientToken": "one-token",
            "tags": { reserved_key.clone(): "test" },
        }),
    )
    .await;
    assert_validation(&refused, "a replayed token carrying a reserved key");

    let replayed = ok(
        &create_task(
            &router,
            json!({
                "targets": [&device],
                "command": { "reboot": {} },
                "clientToken": "one-token",
                "tags": { "Name": "nightly" },
            }),
        )
        .await,
        "a replayed token carrying an acceptable map",
    );
    assert_eq!(
        string(&replayed, "taskId", case),
        string(&first, "taskId", case),
        "case {case:?}: the same clientToken must still name the first task"
    );
    let listed = listed_tasks(&router, case).await;
    assert_eq!(
        listed.len(),
        1,
        "case {case:?}: neither the refusal nor the replay may mint a second task, \
         got {listed:?}"
    );
}
