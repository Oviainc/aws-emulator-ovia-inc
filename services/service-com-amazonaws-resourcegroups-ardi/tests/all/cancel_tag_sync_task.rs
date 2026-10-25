// SPDX-License-Identifier: CC0-1.0

//! `CancelTagSyncTask`, end to end.
//!
//! Every request goes through the same `MultiServiceRouter::dispatch` the served
//! emulator uses (the harness is the `resource_group_lifecycle` suite's `Emulator`), so
//! these exercise the whole restJson1 path: routing, boundary validation, the handler,
//! and serialization.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one row:
//!
//! * cancelling a tag-sync task STOPS it, so the task the ARN addressed is no longer one
//!   this service holds: the read that answered it before answers `NotFoundException`
//!   (404) after, whatever the task synced and whichever region it lived in;
//! * cancelling selects the ONE task the ARN addresses: every other stored task, the
//!   group the cancelled task synced into, and that group's own tasks are untouched, and
//!   the group can host a new task afterwards;
//! * the operation models no not-found error, so cancelling a task this service does not
//!   hold — an already-cancelled one, an ARN nothing was minted for, or a task of another
//!   region — is a success that changes nothing.

use serde_json::{json, Value};

use super::resource_group_lifecycle::{error_code, Emulator};
use super::tag_sync_tasks::{group_arn, role_arn};

/// Every value the modelled `QueryType` enum declares, which is the domain of the
/// `Type` a tag-sync task's stored query can carry.
///
/// Source: `smithy_query(query="shape",shape="QueryType")` on this service's model,
/// which answers `["TAG_FILTERS_1_0", "CLOUDFORMATION_STACK_1_0"]`.
const QUERY_TYPE_VALUES: [&str; 2] = ["TAG_FILTERS_1_0", "CLOUDFORMATION_STACK_1_0"];

/// One tag-sync task to start and then cancel: the rows differ in what the task syncs
/// (a tag key-value pair or a resource query of either declared type), in the values and
/// sizes those members take, in the region the group lives in, and in the group's name —
/// the dimensions the cancellation rule generalises over, since a task is cancelled by
/// its own ARN alone.
#[derive(Debug, Clone)]
struct CancelCase {
    label: &'static str,
    region: &'static str,
    group: &'static str,
    role: &'static str,
    /// The members sent beside `Group` and `RoleArn` to say what the task syncs.
    sync: fn() -> Value,
}

fn cancel_cases() -> Vec<CancelCase> {
    vec![
        CancelCase {
            label: "a tag key-value pair",
            region: "us-east-1",
            group: "cancel-one",
            role: "ResourceGroupsTagSync",
            sync: || json!({ "TagKey": "Stage", "TagValue": "Test" }),
        },
        CancelCase {
            label: "a tag pair whose value is empty, which the model admits (len 0..256)",
            region: "eu-west-1",
            group: "cancel.two_2",
            role: "tag-sync/with-a-path",
            sync: || json!({ "TagKey": "Team", "TagValue": "" }),
        },
        CancelCase {
            label: "a tag-filters resource query with several filters",
            region: "ap-southeast-2",
            group: "cancel-three",
            role: "AnotherTagSyncRole",
            sync: || {
                json!({
                    "ResourceQuery": {
                        "Type": "TAG_FILTERS_1_0",
                        "Query": r#"{"ResourceTypeFilters":["AWS::EC2::Instance"],"TagFilters":[{"Key":"Stage","Values":["Test"]}]}"#,
                    }
                })
            },
        },
        CancelCase {
            label: "a cloudformation-stack resource query",
            region: "us-west-2",
            group: "cancel-four",
            role: "ResourceGroupsTagSync",
            sync: || {
                json!({
                    "ResourceQuery": {
                        "Type": "CLOUDFORMATION_STACK_1_0",
                        "Query": r#"{"StackIdentifier":"arn:aws:cloudformation:us-west-2:000000000000:stack/one/00000000-0000-0000-0000-000000000000"}"#,
                    }
                })
            },
        },
        CancelCase {
            label: "a tag pair carrying the punctuation the modelled TagKey admits",
            region: "us-east-2",
            group: "cancel-five",
            role: "ResourceGroupsTagSync",
            sync: || json!({ "TagKey": "cost:center", "TagValue": "platform engineering" }),
        },
    ]
}

impl CancelCase {
    /// The `StartTagSyncTask` request body for this case, with `suffix` distinguishing
    /// the role the task assumes — a second task of the same group differs only in that.
    fn request(&self, suffix: &str) -> Value {
        let role = role_arn(&format!("{}{suffix}", self.role));
        let mut body = json!({ "Group": self.group, "RoleArn": role });
        let object = body.as_object_mut().expect("a JSON object");
        for (member, value) in (self.sync)().as_object().expect("a JSON object") {
            object.insert(member.clone(), value.clone());
        }
        body
    }
}

/// Start the task `request` describes in `region` and return its minted task ARN.
async fn start_task(emulator: &Emulator, region: &str, request: &Value) -> String {
    let started = emulator
        .ok_in(region, "/start-tag-sync-task", request)
        .await;
    started["TaskArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {request}: expected a TaskArn, got {started}"))
        .to_string()
}

/// `CancelTagSyncTask` for `task_arn` in `region`: the status and the parsed body.
async fn cancel(emulator: &Emulator, region: &str, task_arn: &str) -> (u16, Value) {
    emulator
        .call_in(
            region,
            "/cancel-tag-sync-task",
            &json!({ "TaskArn": task_arn }),
        )
        .await
}

/// `GetTagSyncTask` for `task_arn` in `region`: the status and the parsed body.
async fn read(emulator: &Emulator, region: &str, task_arn: &str) -> (u16, Value) {
    emulator
        .call_in(region, "/get-tag-sync-task", &json!({ "TaskArn": task_arn }))
        .await
}

/// A cancellation answered 200 with the empty body the modelled `Unit` output carries.
fn assert_empty_success(label: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 200,
        "case {label:?}: expected CancelTagSyncTask to answer 200, got {status} {body}",
    );
    assert!(
        body.is_null() || body.as_object().is_some_and(|members| members.is_empty()),
        "case {label:?}: expected the empty body of a modelled Unit output, got {body}",
    );
}

/// Every row's query type is one the model declares, and the table covers both ways a
/// task can sync, so it spans the domain rather than one point of it.
#[test]
fn the_table_spans_both_sync_methods_and_every_declared_query_type() {
    let labels: Vec<&str> = cancel_cases().iter().map(|case| case.label).collect();
    assert!(
        cancel_cases()
            .iter()
            .any(|case| (case.sync)().get("TagKey").is_some()),
        "case \"table\": expected a row syncing a tag key-value pair, got {labels:?}",
    );
    for query_type in QUERY_TYPE_VALUES {
        assert!(
            cancel_cases().iter().any(|case| {
                (case.sync)()["ResourceQuery"]["Type"].as_str() == Some(query_type)
            }),
            "case {query_type:?}: expected a row syncing a query of that declared type, \
             got {labels:?}",
        );
    }
}

/// A cancelled task is no longer held: the read that answered it answers 404 after, the
/// cancellation is idempotent, and the group, its other tasks and its ability to host a
/// new task are untouched.
#[tokio::test]
async fn a_cancelled_task_is_no_longer_held_and_nothing_else_changes() {
    let emulator = Emulator::new();
    for case in cancel_cases() {
        let label = case.label;
        emulator
            .ok_in(case.region, "/groups", &json!({ "Name": case.group }))
            .await;
        // Two tasks of ONE group, so the cancellation has a sibling to leave alone.
        let cancelled_arn = start_task(&emulator, case.region, &case.request("-cancelled")).await;
        let kept_arn = start_task(&emulator, case.region, &case.request("-kept")).await;
        assert_ne!(
            cancelled_arn, kept_arn,
            "case {label:?}: expected two tasks of one group to carry distinct ARNs",
        );
        let before = read(&emulator, case.region, &cancelled_arn).await;
        assert_eq!(
            before.0, 200,
            "case {label:?}: expected the task to be readable before it is cancelled, \
             got {} {}",
            before.0, before.1,
        );

        let (status, body) = cancel(&emulator, case.region, &cancelled_arn).await;
        assert_empty_success(label, status, &body);

        // The cancelled task is gone: the ARN that addressed it addresses nothing.
        let after = read(&emulator, case.region, &cancelled_arn).await;
        assert_eq!(
            (after.0, error_code(&after.1)),
            (404, "NotFoundException"),
            "case {label:?}: expected a cancelled task to be found no more, got {} {}",
            after.0,
            after.1,
        );
        // Cancelling selects the one task the ARN names: its sibling is still stored,
        // and reads back as itself.
        let kept = read(&emulator, case.region, &kept_arn).await;
        assert_eq!(
            (kept.0, kept.1["TaskArn"].as_str()),
            (200, Some(kept_arn.as_str())),
            "case {label:?}: expected the group's other task to survive, got {} {}",
            kept.0,
            kept.1,
        );
        // Cancelling a task does not delete the group it synced into.
        let group = emulator
            .call_in(case.region, "/get-group", &json!({ "Group": case.group }))
            .await;
        let arn = group_arn(case.region, case.group);
        assert_eq!(
            (group.0, group.1["Group"]["GroupArn"].as_str()),
            (200, Some(arn.as_str())),
            "case {label:?}: expected the group to still read back as itself, got {} {}",
            group.0,
            group.1,
        );
        // Cancelling again is a success that changes nothing: the operation models no
        // not-found error, and the task is still gone afterwards.
        let (again_status, again_body) = cancel(&emulator, case.region, &cancelled_arn).await;
        assert_empty_success(label, again_status, &again_body);
        let still_absent = read(&emulator, case.region, &cancelled_arn).await;
        assert_eq!(
            (still_absent.0, error_code(&still_absent.1)),
            (404, "NotFoundException"),
            "case {label:?}: expected a repeated cancellation to leave the task absent, \
             got {} {}",
            still_absent.0,
            still_absent.1,
        );
        // The group can host a new task after the cancellation, and that task gets an
        // ARN of its own rather than the cancelled one.
        let fresh_arn = start_task(&emulator, case.region, &case.request("-fresh")).await;
        assert_ne!(
            fresh_arn, cancelled_arn,
            "case {label:?}: expected a task started after the cancellation to carry its \
             own ARN",
        );
        let fresh = read(&emulator, case.region, &fresh_arn).await;
        assert_eq!(
            (fresh.0, fresh.1["TaskArn"].as_str()),
            (200, Some(fresh_arn.as_str())),
            "case {label:?}: expected the newly started task to be readable, got {} {}",
            fresh.0,
            fresh.1,
        );
    }
}

/// A 26-character id of the form the modelled ARNs are written with (`[a-z0-9]{26}`),
/// built from `seed` so the ids of one test are distinct.
///
/// Source: `smithy_query(query="operation",operation="CancelTagSyncTask")` — `TaskArn` is
/// `…:group/<name>/[a-z0-9]{26}/tag-sync-task/[a-z0-9]{26}$`.
fn modelled_id(seed: char) -> String {
    std::iter::repeat_n(seed, 26).collect()
}

/// A task ARN the store never minted, shaped exactly like the modelled `TagSyncTaskArn`
/// so the boundary validator admits it and the handler decides the answer.
fn unminted_task_arn(region: &str, group: &str, seed: char) -> String {
    format!(
        "{}/{}/tag-sync-task/{}",
        group_arn(region, group),
        modelled_id(seed),
        modelled_id(seed),
    )
}

/// Cancelling a task this service does not hold is a success that changes nothing: an
/// ARN nothing was minted for, an ARN of a group that does not exist, and a stored task
/// addressed through ANOTHER region's endpoint all answer 200 and leave the store alone.
#[tokio::test]
async fn cancelling_a_task_this_service_does_not_hold_changes_nothing() {
    let emulator = Emulator::new();
    emulator.ok("/groups", &json!({ "Name": "cancel-absent" })).await;
    let stored_arn = start_task(
        &emulator,
        "us-east-1",
        &json!({
            "Group": "cancel-absent",
            "RoleArn": role_arn("ResourceGroupsTagSync"),
            "TagKey": "Stage",
            "TagValue": "Test",
        }),
    )
    .await;

    let absent = [
        (
            "an ARN nothing was minted for, on a group that exists",
            "us-east-1",
            unminted_task_arn("us-east-1", "cancel-absent", 'a'),
        ),
        (
            "an ARN nothing was minted for, on a group that does not exist",
            "us-east-1",
            unminted_task_arn("us-east-1", "never-created", 'b'),
        ),
        (
            "an ARN of another account",
            "us-east-1",
            format!(
                "arn:aws:resource-groups:us-east-1:111111111111:group/cancel-absent/{}/tag-sync-task/{}",
                modelled_id('c'),
                modelled_id('c'),
            ),
        ),
        (
            "a stored task addressed through another region's endpoint",
            "eu-west-1",
            stored_arn.clone(),
        ),
        (
            "a stored task addressed through a third region's endpoint",
            "ap-southeast-2",
            stored_arn.clone(),
        ),
    ];
    for (label, region, task_arn) in absent {
        let (status, body) = cancel(&emulator, region, &task_arn).await;
        assert_empty_success(label, status, &body);
        // The task the store DOES hold is untouched by any of them.
        let kept = read(&emulator, "us-east-1", &stored_arn).await;
        assert_eq!(
            (kept.0, kept.1["TaskArn"].as_str()),
            (200, Some(stored_arn.as_str())),
            "case {label:?}: expected the stored task to survive, got {} {}",
            kept.0,
            kept.1,
        );
    }

    // The task's own region still cancels it, which is what the other regions did not.
    let (status, body) = cancel(&emulator, "us-east-1", &stored_arn).await;
    assert_empty_success("the task's own region", status, &body);
    let gone = read(&emulator, "us-east-1", &stored_arn).await;
    assert_eq!(
        (gone.0, error_code(&gone.1)),
        (404, "NotFoundException"),
        "case \"the task's own region\": expected the task to be cancelled, got {} {}",
        gone.0,
        gone.1,
    );
}

/// A `TaskArn` that violates the modelled `TagSyncTaskArn` is refused at the boundary,
/// before the handler runs, so a malformed request never reaches the store.
#[tokio::test]
async fn a_task_arn_outside_the_modelled_shape_is_refused() {
    let emulator = Emulator::new();
    let malformed = [
        ("an empty ARN, shorter than the modelled minimum of 12", String::new()),
        (
            "an ARN with no tag-sync-task segment",
            group_arn("us-east-1", "cancel-malformed"),
        ),
        (
            "an ARN whose task id is not 26 characters",
            format!(
                "{}/{}/tag-sync-task/short",
                group_arn("us-east-1", "cancel-malformed"),
                modelled_id('d'),
            ),
        ),
    ];
    for (label, task_arn) in malformed {
        let (status, body) = cancel(&emulator, "us-east-1", &task_arn).await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected a boundary refusal with status 400, \
             got {status} {body}",
        );
    }
}
