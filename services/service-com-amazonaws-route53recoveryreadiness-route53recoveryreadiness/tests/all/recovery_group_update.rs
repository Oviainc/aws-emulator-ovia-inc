// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateRecoveryGroup`.
//!
//! The rule under test: an update of a recovery group is a WHOLE-MEMBER replacement of
//! its cell list — the submitted list becomes the stored one exactly, in the submitted
//! order and with no merge with what was there; the record's identity (its ARN and its
//! name) and the tags it carries are untouched; every read surface afterwards reports
//! the replacement, including the containment a cell derives from it; an update
//! addressed to a group no record holds changes nothing and is refused HTTP 404
//! `ResourceNotFoundException`; and an update whose body breaks a rule a create is held
//! to is refused HTTP 400 `ValidationException` with the previous list left intact.
//!
//! Every assertion goes over the wire through the same [`MultiServiceRouter`] the
//! production emulator mounts (the harness is shared with `readiness_check_lifecycle.rs`
//! rather than copied), so only the HTTP status, the `x-amzn-errortype` header and the
//! JSON body are inspected: an answer that is not computed from persisted state cannot
//! pass.
//!
//! The tests are tables whose rows differ along what the rule generalises over: the cell
//! COUNT before and after the update (growing, shrinking, emptying, filling, up to the
//! stored ceiling), how the two lists OVERLAP and in which ORDER they are submitted, the
//! name form and length addressed, the tag count the record carries while being updated,
//! the number of updates applied in a row, and which rule a refused body breaks.

use serde_json::{json, Value};
use service_com_amazonaws_route53recoveryreadiness_route53recoveryreadiness::state::{
    MAX_NESTED_CELLS_PER_CELL, MAX_TAGS_PER_RESOURCE,
};

use crate::readiness_check_lifecycle::{Emulator, Reply};

/// The constant, resource-agnostic sentence this service answers a not-found with.
/// Written out here rather than imported, so the test pins the string.
const NOT_FOUND_MESSAGE: &str = "Resource not found";

/// The constant sentence a write is refused with when it would place one cell into a
/// second readiness scope.
const ALREADY_CONTAINED_MESSAGE: &str = "A cell can be contained by only one readiness scope.";

/// `__stringMax64PatternAAZAZ09Z`, the shape `RecoveryGroupName` is reported through, is
/// `length 0..64 pattern ^\A[a-zA-Z0-9_]+\z$` (model:
/// `smithy_query(query="shape", shape="__stringMax64PatternAAZAZ09Z")`).
const NAME_MAX_LENGTH: usize = 64;

/// The ARN AWS documents for a cell. Route 53 Application Recovery Controller readiness
/// is a GLOBAL service, so the region segment is empty:
/// `arn:aws:route53-recovery-readiness::<account>:cell/<name>`. Written out here rather
/// than taken from the handler's helper, so the test pins the format.
fn cell_arn(name: &str) -> String {
    format!(
        "arn:aws:route53-recovery-readiness::{account}:cell/{name}",
        account = engine::EMULATOR_ACCOUNT_ID
    )
}

/// The same for a recovery group:
/// `arn:aws:route53-recovery-readiness::<account>:recovery-group/<name>`.
fn recovery_group_arn(name: &str) -> String {
    format!(
        "arn:aws:route53-recovery-readiness::{account}:recovery-group/{name}",
        account = engine::EMULATOR_ACCOUNT_ID
    )
}

/// `count` distinct cell ARNs, namespaced by `label` so two rows of one table never name
/// the same cell (a cell belongs to one scope, so sharing one would be refused).
fn cell_arns(label: &str, count: usize) -> Vec<String> {
    (0..count)
        .map(|index| cell_arn(&format!("{label}_cell_{index}")))
        .collect()
}

/// `POST /recoverygroups` (model: `CreateRecoveryGroup`).
async fn create_recovery_group(emulator: &Emulator, body: Value) -> Reply {
    emulator.send("POST", "/recoverygroups", Some(body)).await
}

/// `GET /recoverygroups/{RecoveryGroupName}` (model: `GetRecoveryGroup`).
async fn get_recovery_group(emulator: &Emulator, name: &str) -> Reply {
    emulator
        .send("GET", &format!("/recoverygroups/{name}"), None)
        .await
}

/// `PUT /recoverygroups/{RecoveryGroupName}` (model: `UpdateRecoveryGroup`).
async fn update_recovery_group(emulator: &Emulator, name: &str, cells: &[String]) -> Reply {
    emulator
        .send(
            "PUT",
            &format!("/recoverygroups/{name}"),
            Some(json!({ "cells": cells })),
        )
        .await
}

/// The entry `ListRecoveryGroups` reports for `name`, or `None` when the listing does
/// not hold it.
async fn listed_entry(emulator: &Emulator, name: &str) -> Option<Value> {
    emulator
        .send("GET", "/recoverygroups?maxResults=1000", None)
        .await
        .field("recoveryGroups")
        .and_then(Value::as_array)
        .and_then(|items| {
            items
                .iter()
                .find(|item| item.get("recoveryGroupName").and_then(Value::as_str) == Some(name))
                .cloned()
        })
}

/// A reply's string-list member, with an omitted member read as the empty list: the wire
/// may carry `[]` or nothing at all for a group that holds no cell.
fn strings(reply: &Reply, member: &str) -> Vec<String> {
    reply
        .field(member)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| item.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// The same, for one entry of a list response.
fn entry_strings(entry: &Value, member: &str) -> Vec<String> {
    entry
        .get(member)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| item.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Store a recovery group and assert the create succeeded.
async fn seed(emulator: &Emulator, name: &str, cells: &[String], tags: Option<Value>) {
    let mut body = json!({ "recoveryGroupName": name, "cells": cells });
    if let Some(tags) = tags {
        body["tags"] = tags;
    }
    let reply = create_recovery_group(emulator, body).await;
    assert_eq!(
        reply.status, 200,
        "case {name:?}: expected CreateRecoveryGroup to answer HTTP 200, got {} / {}",
        reply.status, reply.body
    );
}

/// Every row replaces a stored group's cell list, and the replacement is what the update
/// reports, what `GetRecoveryGroup` reads back and what `ListRecoveryGroups` reports —
/// while the ARN and the name stay the record's identity.
#[tokio::test]
async fn an_update_replaces_the_cell_list_everywhere_the_record_is_read() {
    // Rows differ along the cell count BEFORE and AFTER the update: growing, shrinking,
    // emptying, filling an empty group, and the stored ceiling in both directions.
    let rows: Vec<(&str, usize, usize)> = vec![
        ("empty to one", 0, 1),
        ("one to empty", 1, 0),
        ("one to one", 1, 1),
        ("one to several", 1, 5),
        ("several to one", 6, 1),
        ("several to several", 3, 7),
        ("empty to empty", 0, 0),
        ("two to the stored ceiling", 2, MAX_NESTED_CELLS_PER_CELL),
        ("the stored ceiling to one", MAX_NESTED_CELLS_PER_CELL, 1),
    ];

    let emulator = Emulator::new();
    for (index, (case, before, after)) in rows.into_iter().enumerate() {
        let name = format!("update_group_{index}");
        seed(&emulator, &name, &cell_arns(&format!("b{index}"), before), None).await;

        let replacement = cell_arns(&format!("a{index}"), after);
        let updated = update_recovery_group(&emulator, &name, &replacement).await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected UpdateRecoveryGroup to answer HTTP 200, got {} / {}",
            updated.status, updated.body
        );
        assert_eq!(
            strings(&updated, "cells"),
            replacement,
            "case {case:?}: expected the replacement cell list back, got {}",
            updated.body
        );
        assert_eq!(
            updated.string("recoveryGroupArn"),
            Some(recovery_group_arn(&name)),
            "case {case:?}: expected the record's ARN unchanged, got {:?}",
            updated.string("recoveryGroupArn")
        );
        assert_eq!(
            updated.string("recoveryGroupName").as_deref(),
            Some(name.as_str()),
            "case {case:?}: expected the record's name unchanged, got {:?}",
            updated.string("recoveryGroupName")
        );

        // The replacement is PERSISTED: the reader sees exactly what the update reported.
        let fetched = get_recovery_group(&emulator, &name).await;
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: expected the updated record to be readable, got {} / {}",
            fetched.status, fetched.body
        );
        assert_eq!(
            strings(&fetched, "cells"),
            replacement,
            "case {case:?}: expected GetRecoveryGroup to report the replacement, got {}",
            fetched.body
        );
        assert_eq!(
            fetched.string("recoveryGroupArn"),
            Some(recovery_group_arn(&name)),
            "case {case:?}: expected the read ARN unchanged, got {:?}",
            fetched.string("recoveryGroupArn")
        );

        let entry = listed_entry(&emulator, &name)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: expected the record to be listed"));
        assert_eq!(
            entry_strings(&entry, "cells"),
            replacement,
            "case {case:?}: expected the listing to report the replacement, got {entry}"
        );
    }
}

/// An update does not merge, and it keeps the submitted ORDER: whatever was stored is
/// gone. Rows differ in how the two lists overlap and how they are ordered, which is how
/// a merge or a re-sort would show itself.
#[tokio::test]
async fn an_update_replaces_rather_than_merges_the_cell_list() {
    let shared = cell_arns("shared", 3);
    let rows: Vec<(&str, Vec<String>, Vec<String>)> = vec![
        (
            "a disjoint replacement",
            cell_arns("disjoint_before", 2),
            cell_arns("disjoint_after", 1),
        ),
        (
            "a replacement that keeps one entry",
            vec![shared[0].clone(), shared[1].clone()],
            vec![shared[1].clone()],
        ),
        (
            "the same entries in the opposite order",
            shared.clone(),
            shared.iter().rev().cloned().collect(),
        ),
        (
            "a replacement that adds to what was there",
            vec![shared[0].clone()],
            shared.clone(),
        ),
        (
            "an emptied list",
            cell_arns("emptied", 2),
            Vec::new(),
        ),
        (
            "a list that repeats one cell",
            cell_arns("repeated", 1),
            vec![
                cell_arn("repeated_cell_0"),
                cell_arn("repeated_cell_0"),
            ],
        ),
    ];

    let emulator = Emulator::new();
    for (index, (case, before, after)) in rows.into_iter().enumerate() {
        let name = format!("replace_group_{index}");
        seed(&emulator, &name, &before, None).await;

        let updated = update_recovery_group(&emulator, &name, &after).await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected HTTP 200, got {} / {}",
            updated.status, updated.body
        );

        let fetched = get_recovery_group(&emulator, &name).await;
        let stored = strings(&fetched, "cells");
        assert_eq!(
            stored, after,
            "case {case:?}: expected the stored list to be exactly the replacement, got {stored:?}"
        );
        assert_eq!(
            stored.len(),
            after.len(),
            "case {case:?}: expected {} stored cells, got {}",
            after.len(),
            stored.len()
        );

        // Each row is a FRESH group, so the cells one row used are released for the next:
        // emptying the group here is what keeps the rows independent.
        let cleared = update_recovery_group(&emulator, &name, &[]).await;
        assert_eq!(
            cleared.status, 200,
            "case {case:?}: expected the group to be emptied, got {} / {}",
            cleared.status, cleared.body
        );
    }
}

/// Applying the same body twice leaves the same record, and a chain of updates leaves
/// the LAST one. Rows differ in how many updates are applied in a row.
#[tokio::test]
async fn repeated_updates_leave_the_last_one_stored() {
    let emulator = Emulator::new();
    for rounds in [1usize, 2, 3, 5] {
        let name = format!("repeat_group_{rounds}");
        seed(&emulator, &name, &cell_arns(&name, 2), None).await;

        let mut last: Option<Vec<String>> = None;
        for round in 0..rounds {
            let cells = cell_arns(&format!("{name}_r{round}"), round + 1);
            // The same body is applied twice per round, so an update that accumulated
            // state would show up as a difference between the two answers.
            let first = update_recovery_group(&emulator, &name, &cells).await;
            let again = update_recovery_group(&emulator, &name, &cells).await;
            assert_eq!(
                (first.status, again.status),
                (200, 200),
                "case {rounds} rounds, round {round}: expected both updates to answer HTTP 200, got {} / {}",
                first.status,
                again.status
            );
            assert_eq!(
                again.body, first.body,
                "case {rounds} rounds, round {round}: expected an idempotent update, got {} then {}",
                first.body, again.body
            );
            last = Some(cells);
        }

        let expected = last.expect("every row applies at least one update");
        let fetched = get_recovery_group(&emulator, &name).await;
        assert_eq!(
            strings(&fetched, "cells"),
            expected,
            "case {rounds} rounds: expected the last list stored, got {}",
            fetched.body
        );
    }
}

/// The tags a record carries are not part of an update's request, so an update leaves
/// them exactly as they were. Rows differ in the tag count, up to the stored ceiling.
#[tokio::test]
async fn an_update_leaves_the_records_tags_alone() {
    let emulator = Emulator::new();
    for tag_count in [0usize, 1, 2, 7, MAX_TAGS_PER_RESOURCE] {
        let name = format!("tagged_group_{tag_count}");
        let tags: serde_json::Map<String, Value> = (0..tag_count)
            .map(|index| (format!("key_{index}"), json!(format!("value_{index}"))))
            .collect();
        let tags = Value::Object(tags);
        seed(
            &emulator,
            &name,
            &cell_arns(&name, 2),
            Some(tags.clone()),
        )
        .await;

        let before = get_recovery_group(&emulator, &name).await;
        let updated = update_recovery_group(&emulator, &name, &cell_arns(&format!("{name}_x"), 3))
            .await;
        assert_eq!(
            updated.status, 200,
            "case {tag_count} tags: expected HTTP 200, got {} / {}",
            updated.status, updated.body
        );

        // An untagged record reports no tags member at all.
        let expected = if tag_count == 0 { None } else { Some(tags) };
        assert_eq!(
            updated.field("tags").cloned(),
            expected,
            "case {tag_count} tags: expected the stored tags reported unchanged, got {:?}",
            updated.field("tags")
        );
        assert_eq!(
            get_recovery_group(&emulator, &name)
                .await
                .field("tags")
                .cloned(),
            before.field("tags").cloned(),
            "case {tag_count} tags: expected the stored tags untouched by the update"
        );
    }
}

/// The containment a CELL derives is read off the group's stored list, so an update that
/// takes a cell in or out moves that cell's `ParentReadinessScopes` with it. Rows differ
/// in how many cells the group holds across the move.
#[tokio::test]
async fn an_update_moves_the_containment_the_member_cells_report() {
    let emulator = Emulator::new();
    for member_count in [1usize, 2, 4] {
        let group = format!("scope_group_{member_count}");
        let cells: Vec<String> = (0..member_count)
            .map(|index| format!("scope_cell_{member_count}_{index}"))
            .collect();
        for cell in &cells {
            let created = emulator
                .send("POST", "/cells", Some(json!({ "cellName": cell, "cells": [] })))
                .await;
            assert_eq!(
                created.status, 200,
                "case {member_count} members: expected CreateCell({cell}) to answer HTTP 200, got {} / {}",
                created.status, created.body
            );
        }
        seed(&emulator, &group, &[], None).await;

        // Before the update nothing contains the cells.
        for cell in &cells {
            let read = emulator.send("GET", &format!("/cells/{cell}"), None).await;
            assert_eq!(
                strings(&read, "parentReadinessScopes"),
                Vec::<String>::new(),
                "case {member_count} members: expected {cell} to report no scope yet, got {}",
                read.body
            );
        }

        let arns: Vec<String> = cells.iter().map(|cell| cell_arn(cell)).collect();
        let updated = update_recovery_group(&emulator, &group, &arns).await;
        assert_eq!(
            updated.status, 200,
            "case {member_count} members: expected HTTP 200, got {} / {}",
            updated.status, updated.body
        );
        for cell in &cells {
            let read = emulator.send("GET", &format!("/cells/{cell}"), None).await;
            assert_eq!(
                strings(&read, "parentReadinessScopes"),
                vec![recovery_group_arn(&group)],
                "case {member_count} members: expected {cell} to report the group as its scope, got {}",
                read.body
            );
        }

        // Taking the cells back out releases the scope again.
        let emptied = update_recovery_group(&emulator, &group, &[]).await;
        assert_eq!(
            emptied.status, 200,
            "case {member_count} members: expected the emptying update to answer HTTP 200, got {} / {}",
            emptied.status, emptied.body
        );
        for cell in &cells {
            let read = emulator.send("GET", &format!("/cells/{cell}"), None).await;
            assert_eq!(
                strings(&read, "parentReadinessScopes"),
                Vec::<String>::new(),
                "case {member_count} members: expected {cell} to report no scope again, got {}",
                read.body
            );
        }
    }
}

/// An update addressed to a group no record holds is refused 404 with this service's
/// constant sentence, and nothing is stored for it. Rows differ in the FORM of the
/// missing name.
#[tokio::test]
async fn updating_a_group_that_does_not_exist_is_refused_and_stores_nothing() {
    let rows: Vec<(&str, String)> = vec![
        ("a plain absent name", "absent_group".to_string()),
        ("a name that differs only in case", "PRESENT_GROUP".to_string()),
        ("a name that is a prefix of a stored one", "present".to_string()),
        ("a name with a trailing underscore", "present_group_".to_string()),
        // A name AT the published length bound is still just a name no record holds.
        ("a name at the reported length bound", "n".repeat(NAME_MAX_LENGTH)),
    ];

    let emulator = Emulator::new();
    let present = cell_arns("present", 2);
    seed(&emulator, "present_group", &present, None).await;

    for (case, missing) in rows {
        let missing = missing.as_str();
        let refused = update_recovery_group(&emulator, missing, &cell_arns("ghost", 1)).await;
        assert_eq!(
            refused.status, 404,
            "case {case:?} ({missing:?}): expected HTTP 404, got {} / {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "ResourceNotFoundException",
            "case {case:?} ({missing:?}): expected ResourceNotFoundException, got {:?} / {}",
            refused.error_code(),
            refused.body
        );
        assert_eq!(
            refused.string("message").as_deref(),
            Some(NOT_FOUND_MESSAGE),
            "case {case:?} ({missing:?}): expected the service's not-found sentence, got {:?}",
            refused.string("message")
        );
        assert_eq!(
            get_recovery_group(&emulator, missing).await.status,
            404,
            "case {case:?} ({missing:?}): a refused update must not create the record"
        );
        assert!(
            listed_entry(&emulator, missing).await.is_none(),
            "case {case:?} ({missing:?}): a refused update must not be listed"
        );
    }

    // The group that DOES exist is untouched by every refusal above.
    let survivor = get_recovery_group(&emulator, "present_group").await;
    assert_eq!(
        strings(&survivor, "cells"),
        present,
        "case {:?}: expected the stored record untouched by the refusals, got {}",
        "survivor",
        survivor.body
    );
}

/// An update is held to the rules a create is held to, and a refused update leaves the
/// previous record exactly as it was. Rows differ in which rule the body breaks.
#[tokio::test]
async fn a_refused_update_leaves_the_previous_record_intact() {
    let emulator = Emulator::new();

    // The cell list one accepted record may carry is bounded, so a list one past the
    // ceiling is refused — while the ceiling itself is accepted (covered by the
    // replacement table above).
    let name = "bounded_group";
    let original = cell_arns("bounded", 2);
    seed(&emulator, name, &original, None).await;
    let over = cell_arns("over", MAX_NESTED_CELLS_PER_CELL + 1);
    let refused = update_recovery_group(&emulator, name, &over).await;
    assert_eq!(
        refused.status, 400,
        "case {:?}: expected HTTP 400, got {} / {}",
        "one cell past the stored ceiling", refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        "ValidationException",
        "case {:?}: expected ValidationException, got {:?} / {}",
        "one cell past the stored ceiling",
        refused.error_code(),
        refused.body
    );
    assert_eq!(
        strings(&get_recovery_group(&emulator, name).await, "cells"),
        original,
        "case {:?}: expected the previous list to survive the refusal",
        "one cell past the stored ceiling"
    );

    // A cell belongs to ONE readiness scope, so an update may not claim a cell another
    // record already holds. Rows differ in which kind of record holds it.
    let taken_by_group = cell_arn("taken_by_group");
    let taken_by_cell = cell_arn("taken_by_cell");
    seed(
        &emulator,
        "owning_group",
        std::slice::from_ref(&taken_by_group),
        None,
    )
    .await;
    let parent = emulator
        .send(
            "POST",
            "/cells",
            Some(json!({ "cellName": "owning_cell", "cells": [taken_by_cell.clone()] })),
        )
        .await;
    assert_eq!(
        parent.status, 200,
        "expected CreateCell(owning_cell) to answer HTTP 200, got {} / {}",
        parent.status, parent.body
    );

    let claimant = "claiming_group";
    let kept = cell_arns("kept", 1);
    seed(&emulator, claimant, &kept, None).await;
    for (case, contested) in [
        ("a cell another recovery group holds", taken_by_group),
        ("a cell a parent cell nests", taken_by_cell),
    ] {
        let refused = update_recovery_group(&emulator, claimant, &[contested]).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400, got {} / {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?} / {}",
            refused.error_code(),
            refused.body
        );
        assert_eq!(
            refused.string("message").as_deref(),
            Some(ALREADY_CONTAINED_MESSAGE),
            "case {case:?}: expected the one-scope sentence, got {:?}",
            refused.string("message")
        );
        assert_eq!(
            strings(&get_recovery_group(&emulator, claimant).await, "cells"),
            kept,
            "case {case:?}: expected the previous list to survive the refusal"
        );
    }

    // Re-writing the containment this SAME group already has is not a second scope, so
    // it is accepted.
    let again = update_recovery_group(&emulator, claimant, &kept).await;
    assert_eq!(
        again.status, 200,
        "case {:?}: expected HTTP 200, got {} / {}",
        "re-writing the group's own containment", again.status, again.body
    );
}
