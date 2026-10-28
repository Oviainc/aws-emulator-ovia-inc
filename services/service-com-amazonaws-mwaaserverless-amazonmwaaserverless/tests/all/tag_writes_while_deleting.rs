// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the tag writers against a workflow whose teardown has started.
//!
//! RULE under test: a workflow whose teardown has already started is on its way out and
//! accepts no change, so EVERY operation that would write to it is refused and writes
//! nothing — the tag writers included. `UpdateWorkflow` and `StartWorkflowRun` refuse
//! such a write with the modelled `ConflictException` ("the resource is in a state that
//! prevents the requested operation", `smithy_query(query="shape",
//! shape="ConflictException")`); `TagResource` and `UntagResource` model no conflict
//! (`smithy_query(query="operation", operation="TagResource,UntagResource")` declares
//! `AccessDenied | InternalServer | OperationTimeout | ResourceNotFound | Throttling |
//! Validation`), so they refuse it with the error they do model for a request
//! "inconsistent with … service requirements" — the modelled `ValidationException`, HTTP
//! 400, whose `Reason` is `other` because no member of the request is at fault
//! (`smithy_query(query="shape", shape="ValidationExceptionReason")` declares
//! `unknownOperation | cannotParse | fieldValidationFailed | other`).
//!
//! The gate is on the RESOURCE's state, not on what the payload would have done: a
//! request that would have changed nothing (an empty `Tags` map, a key the resource does
//! not carry) is refused exactly like one that would have changed something, the same
//! way an update refuses regardless of the configuration it carries.
//!
//! The table spans the class the rule generalises over: which writer is called, how many
//! pairs or keys the request carries (the published `Tags` minimum of 0 entries, one,
//! several, and the published `TagKeys` maximum of 50), whether each key is one the
//! resource already carries, the resulting entry count up to the modelled ceiling of 50,
//! and the form of the keys and values (plain, punctuated, upper-case, non-ASCII, and the
//! empty value `TagValue`'s minimum admits). Each row runs the SAME request twice: once
//! against a `READY` workflow, where it must succeed, so the refusal below can only come
//! from the resource's state; and once against a workflow in `DELETING`, where it must be
//! refused with nothing written. Every tag set is read back through the implemented
//! `ListTagsForResource`, so what is asserted is what a client sees.

use std::time::{Duration, SystemTime};

use service_com_amazonaws_mwaaserverless_amazonmwaaserverless::state::{
    MAX_TAGS_PER_RESOURCE, WORKFLOW_DELETED_AFTER, WORKFLOW_STATUS_DELETING,
};

use crate::missing_resource_lookups::{LOOKUP_MISS_STATUS, WORKFLOW_MISSING};
use crate::resource_tag_listing::{generated_tags, list_tags, workflow_with_tags};
use crate::workflow_deletion_teardown::with_workflow;
use crate::workflow_lifecycle::Harness;

/// The HTTP status the model gives `ValidationException`
/// (`smithy_query(query="shape", shape="ValidationException")`: `@httpError(400)`).
const VALIDATION_STATUS: u16 = 400;

/// The `ValidationExceptionReason` a refusal no request member is at fault for carries.
const REASON_OTHER: &str = "other";

/// `TagKeys`' published entry maximum (`smithy_query(query="shape", shape="TagKeys")`:
/// `@length(min: 1, max: 50)`).
const MAX_TAG_KEYS: usize = 50;

/// One row: a label, the tag set the resource is created with, the writer the row calls,
/// and the member that writer carries beside `ResourceArn` (`Tags` for `TagResource`,
/// `TagKeys` for `UntagResource`).
struct Case {
    label: String,
    initial: serde_json::Value,
    operation: &'static str,
    member: &'static str,
    payload: serde_json::Value,
}

fn tagging(label: &str, initial: serde_json::Value, tags: serde_json::Value) -> Case {
    Case {
        label: label.to_string(),
        initial,
        operation: "TagResource",
        member: "Tags",
        payload: tags,
    }
}

fn untagging(label: &str, initial: serde_json::Value, keys: &[&str]) -> Case {
    Case {
        label: label.to_string(),
        initial,
        operation: "UntagResource",
        member: "TagKeys",
        payload: serde_json::Value::Array(
            keys.iter()
                .map(|key| serde_json::Value::String((*key).to_string()))
                .collect(),
        ),
    }
}

/// The keys of a `Tags` map, as a `TagKeys` list.
fn all_keys(tags: &serde_json::Value) -> serde_json::Value {
    serde_json::Value::Array(
        tags.as_object()
            .map(|members| {
                members
                    .keys()
                    .map(|key| serde_json::Value::String(key.clone()))
                    .collect()
            })
            .unwrap_or_default(),
    )
}

/// Every row of the table.
fn cases() -> Vec<Case> {
    let empty = serde_json::json!({});
    // One short of the ceiling, so the row below that fills it to exactly
    // `MAX_TAGS_PER_RESOURCE` succeeds on a `READY` resource.
    let nearly_full = generated_tags(MAX_TAGS_PER_RESOURCE - 1);
    let full = generated_tags(MAX_TAG_KEYS);
    vec![
        tagging(
            "one new key onto a resource carrying none",
            empty.clone(),
            serde_json::json!({"env": "prod"}),
        ),
        tagging(
            "overwriting the value of a key the resource carries",
            serde_json::json!({"env": "prod"}),
            serde_json::json!({"env": "staging"}),
        ),
        tagging(
            "several keys at once, punctuated and upper-case and non-ASCII",
            serde_json::json!({"env": "prod"}),
            serde_json::json!({
                "cost.center-01": "finance",
                "OWNER": "alice",
                "équipe": "données",
                "blank": "",
            }),
        ),
        tagging(
            // `Tags` is `@length(min: 0, …)`: a request carrying no pair is legal, and
            // would have changed nothing.
            "an empty Tags map, the published minimum of 0 entries",
            serde_json::json!({"env": "prod"}),
            empty.clone(),
        ),
        tagging(
            &format!("the pair that fills the set to its ceiling of {MAX_TAGS_PER_RESOURCE}"),
            nearly_full,
            serde_json::json!({"last": "one"}),
        ),
        untagging(
            "one of the keys the resource carries",
            serde_json::json!({"env": "prod", "team": "data"}),
            &["env"],
        ),
        untagging(
            "every key the resource carries",
            serde_json::json!({"env": "prod", "team": "data"}),
            &["env", "team"],
        ),
        untagging(
            // A key the resource does not carry would have changed nothing either.
            "a key the resource does not carry",
            serde_json::json!({"env": "prod"}),
            &["absent"],
        ),
        untagging(
            "a punctuated and a non-ASCII key",
            serde_json::json!({"cost.center-01": "finance", "équipe": "données"}),
            &["cost.center-01", "équipe"],
        ),
        Case {
            label: format!("{MAX_TAG_KEYS} keys, the published TagKeys maximum"),
            initial: full.clone(),
            operation: "UntagResource",
            member: "TagKeys",
            payload: all_keys(&full),
        },
    ]
}

/// The row's request, aimed at `arn`.
fn request(case: &Case, arn: &str) -> serde_json::Value {
    let mut body = serde_json::json!({"ResourceArn": arn});
    body[case.member] = case.payload.clone();
    body
}

/// Create a workflow carrying `tags`, delete it, and hold the removal deadline out of
/// reach so the call under test is asserted against the `DELETING` phase rather than
/// against an already removed record. Answers its ARN.
async fn workflow_being_deleted(
    harness: &Harness,
    label: &str,
    name: &str,
    tags: serde_json::Value,
) -> String {
    let arn = workflow_with_tags(harness, label, name, Some(tags)).await;
    let (status, deleted) = harness
        .call("DeleteWorkflow", serde_json::json!({"WorkflowArn": arn}))
        .await;
    assert_eq!(
        status, 200,
        "case {label:?}: expected DeleteWorkflow to succeed, got {status} {deleted:?}"
    );
    with_workflow(harness, &arn, |workflow| {
        workflow.deletion_due_at = Some(SystemTime::now() + Duration::from_secs(3_600));
    });
    let (status, fetched) = harness
        .call("GetWorkflow", serde_json::json!({"WorkflowArn": arn}))
        .await;
    assert_eq!(
        status, 200,
        "case {label:?}: a workflow being deleted is still readable, got {status} {fetched:?}"
    );
    assert_eq!(
        fetched["WorkflowStatus"].as_str(),
        Some(WORKFLOW_STATUS_DELETING),
        "case {label:?}: expected the workflow to be {WORKFLOW_STATUS_DELETING}, got {:?}",
        fetched["WorkflowStatus"]
    );
    arn
}

#[tokio::test]
async fn a_workflow_being_deleted_accepts_no_tag_write() {
    for (index, case) in cases().into_iter().enumerate() {
        // A fresh emulator per row, so one row's resources cannot answer another's call.
        let harness = Harness::new("us-west-2");
        let label = &case.label;

        // The SAME request against a READY workflow: it must succeed, so every refusal
        // below is caused by the resource's state and not by what the request carries.
        let ready = workflow_with_tags(
            &harness,
            label,
            &format!("ready{index}"),
            Some(case.initial.clone()),
        )
        .await;
        let (status, accepted) = harness.call(case.operation, request(&case, &ready)).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected {} to succeed on a READY workflow, got {status} \
             {accepted:?}",
            case.operation
        );

        let deleting = workflow_being_deleted(
            &harness,
            label,
            &format!("deleting{index}"),
            case.initial.clone(),
        )
        .await;
        let (status, refused) = harness.call(case.operation, request(&case, &deleting)).await;
        assert_eq!(
            status, VALIDATION_STATUS,
            "case {label:?}: a workflow being deleted accepts no change, so {} must answer \
             {VALIDATION_STATUS}, got {status} {refused:?}",
            case.operation
        );
        assert!(
            refused["__type"]
                .as_str()
                .is_some_and(|kind| kind.contains("ValidationException")),
            "case {label:?}: expected a ValidationException, got {:?}",
            refused["__type"]
        );
        assert_eq!(
            refused["Reason"].as_str(),
            Some(REASON_OTHER),
            "case {label:?}: no request member is at fault, so the reason must be \
             {REASON_OTHER:?}, got {:?}",
            refused["Reason"]
        );
        let message = refused["Message"].as_str().unwrap_or_default();
        assert!(
            message.contains(&deleting) && message.contains(WORKFLOW_STATUS_DELETING),
            "case {label:?}: expected a message naming the resource and its status, got \
             {message:?}"
        );

        // Nothing was written: the tag set a client reads back is the one the resource
        // carried before the refused call.
        let (status, tags) = list_tags(&harness, &deleting).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected ListTagsForResource to succeed, got {status} {tags:?}"
        );
        assert_eq!(
            tags, case.initial,
            "case {label:?}: a refused write must leave the tag set alone, expected {:?}, got \
             {tags:?}",
            case.initial
        );
        // And the refusal changed nothing else about the record either.
        let (status, fetched) = harness
            .call("GetWorkflow", serde_json::json!({"WorkflowArn": deleting}))
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected GetWorkflow to succeed, got {status} {fetched:?}"
        );
        assert_eq!(
            fetched["WorkflowStatus"].as_str(),
            Some(WORKFLOW_STATUS_DELETING),
            "case {label:?}: the refused write must leave the teardown running, got {:?}",
            fetched["WorkflowStatus"]
        );
    }
}

#[tokio::test]
async fn a_workflow_being_deleted_still_reports_the_tag_set_it_carries() {
    // The gate is on the WRITERS: reporting the tags of a workflow on its way out is not
    // a change, so the modelled reader keeps answering until the record goes.
    let harness = Harness::new("eu-west-1");
    let carried = serde_json::json!({"env": "prod", "team": "data"});
    let arn = workflow_being_deleted(&harness, "read while deleting", "readable", carried.clone())
        .await;

    let (status, tags) = list_tags(&harness, &arn).await;
    assert_eq!(
        status, 200,
        "expected ListTagsForResource to succeed for a workflow being deleted, got {status} \
         {tags:?}"
    );
    assert_eq!(
        tags, carried,
        "expected the tag set the workflow carries, got {tags:?}"
    );
}

#[tokio::test]
async fn a_removed_workflow_is_a_lookup_miss_rather_than_the_state_refusal() {
    // Once the removal the clock owes falls due the record is gone, so the writers answer
    // the lookup miss every absent resource gets — a different refusal from the one a
    // workflow still tearing down gets.
    for (operation, member, payload) in [
        ("TagResource", "Tags", serde_json::json!({"env": "prod"})),
        ("UntagResource", "TagKeys", serde_json::json!(["keep"])),
    ] {
        let harness = Harness::new("us-east-1");
        let arn = workflow_with_tags(
            &harness,
            operation,
            "removable",
            Some(serde_json::json!({"keep": "until-removed"})),
        )
        .await;
        let (status, deleted) = harness
            .call("DeleteWorkflow", serde_json::json!({"WorkflowArn": arn}))
            .await;
        assert_eq!(
            status, 200,
            "case {operation:?}: expected DeleteWorkflow to succeed, got {status} {deleted:?}"
        );
        tokio::time::sleep(WORKFLOW_DELETED_AFTER + Duration::from_millis(200)).await;

        let mut body = serde_json::json!({"ResourceArn": arn});
        body[member] = payload;
        let (status, refused) = harness.call(operation, body).await;
        assert_eq!(
            status, LOOKUP_MISS_STATUS,
            "case {operation:?}: expected HTTP {LOOKUP_MISS_STATUS} for a removed resource, got \
             {status} {refused:?}"
        );
        assert_eq!(
            refused["Message"].as_str(),
            Some(WORKFLOW_MISSING),
            "case {operation:?}: expected the workflow-level message, got {:?}",
            refused["Message"]
        );
    }
}
