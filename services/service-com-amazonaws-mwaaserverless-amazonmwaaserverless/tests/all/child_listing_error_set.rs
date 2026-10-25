// SPDX-License-Identifier: CC0-1.0

//! What an operation of this service may answer when the workflow a request names is
//! not one the endpoint holds.
//!
//! RULE under test: an operation answers only the error shapes its OWN modelled error
//! set declares, so an operation whose set declares no not-found error reports a parent
//! resource the endpoint does not hold as the empty page its output models — never as an
//! error shape outside that set, which an SDK client has no modelled type to map onto.
//! Both facts below are read from the Smithy model at run time rather than written in
//! here, so the expectation follows the model: the child-resource listings
//! (`ListWorkflowRuns`, `ListWorkflowVersions`) declare no `ResourceNotFoundException`
//! and the single-resource operations that resolve the same ARN (`GetWorkflow`,
//! `GetWorkflowRun`, `ListTagsForResource`) do.
//!
//! The table spans what the rule generalises over: which operation is called (both
//! listings, and the contrasting single-resource reads), why the parent is absent (never
//! created, created-then-torn-down, held by another region's endpoint), the form,
//! length, region and account of the absent ARN, and — for the paginated listings — the
//! page size at each published `MaxResults` bound and whether a version filter is in
//! play. The control rows are a workflow the endpoint DOES hold, so an empty page is
//! shown to be computed from what is stored rather than always answered.
//!
//! `ListTaskInstances` is the third child-resource listing and declares no not-found
//! error either; the suite `task_instance_listing` carries the rule onto it, over the
//! absent workflow, the absent run and the run another region's endpoint holds.

use std::time::Duration;

use service_com_amazonaws_mwaaserverless_amazonmwaaserverless::state::{
    LOOKUP_MISS_STATUS, WORKFLOW_DELETED_AFTER,
};

use crate::missing_resource_lookups::ghost_arns;
use crate::validation_errors::smithy_model;
use crate::workflow_lifecycle::Harness;
use crate::workflow_run_listing::{created_workflow, started_run};

/// The namespace every shape of this service is declared in.
const NAMESPACE: &str = "com.amazonaws.mwaaserverless";

/// The error shape a lookup miss carries where the operation declares it.
const NOT_FOUND: &str = "ResourceNotFoundException";

/// The region the workflows under test are created at.
const HOME: &str = "us-east-1";

/// A region whose endpoint holds none of them.
const FOREIGN: &str = "eu-west-1";

/// How many runs the control workflow is given, so a non-empty page is a page of more
/// than one item.
const CONTROL_RUNS: usize = 2;

/// A `WorkflowVersion` of the modelled form (`^.+$` for the `VersionId` selector,
/// `smithy_query(query="operation", operation="ListWorkflowRuns")`) that no run carries.
fn ghost_version() -> String {
    "f".repeat(32)
}

/// One operation under test: how to build its request for a workflow ARN and a page
/// size, and the output list member it reports a page in, when it has one.
struct OperationCase {
    /// What this row varies, for the failure message.
    label: &'static str,
    operation: &'static str,
    /// The `@paginated` `items` member, for the listings; `None` for an operation that
    /// reports a single resource and so has no page.
    items: Option<&'static str>,
    /// `(WorkflowArn, MaxResults)` -> request body. An operation that takes no
    /// `MaxResults` ignores the second argument.
    body: fn(&str, Option<i64>) -> serde_json::Value,
}

/// Insert `MaxResults` into a body when the case asks for a page size.
fn with_page_size(mut body: serde_json::Value, max_results: Option<i64>) -> serde_json::Value {
    if let Some(size) = max_results {
        body["MaxResults"] = serde_json::Value::from(size);
    }
    body
}

/// Every operation this suite drives: the two implemented child-resource listings,
/// whose error set declares no not-found error, and the single-resource operations that
/// resolve the very same `WorkflowArn` and whose error sets do.
fn operation_cases() -> Vec<OperationCase> {
    vec![
        OperationCase {
            label: "ListWorkflowRuns",
            operation: "ListWorkflowRuns",
            items: Some("WorkflowRuns"),
            body: |arn, size| with_page_size(serde_json::json!({"WorkflowArn": arn}), size),
        },
        OperationCase {
            label: "ListWorkflowRuns narrowed to one version",
            operation: "ListWorkflowRuns",
            items: Some("WorkflowRuns"),
            body: |arn, size| {
                with_page_size(
                    serde_json::json!({
                        "WorkflowArn": arn,
                        "WorkflowVersion": ghost_version(),
                    }),
                    size,
                )
            },
        },
        OperationCase {
            label: "ListWorkflowVersions",
            operation: "ListWorkflowVersions",
            items: Some("WorkflowVersions"),
            body: |arn, size| with_page_size(serde_json::json!({"WorkflowArn": arn}), size),
        },
        OperationCase {
            label: "GetWorkflow",
            operation: "GetWorkflow",
            items: None,
            body: |arn, _size| serde_json::json!({"WorkflowArn": arn}),
        },
        OperationCase {
            label: "GetWorkflowRun",
            operation: "GetWorkflowRun",
            items: None,
            body: |arn, _size| serde_json::json!({"WorkflowArn": arn, "RunId": "run-01"}),
        },
        OperationCase {
            label: "ListTagsForResource",
            operation: "ListTagsForResource",
            items: None,
            body: |arn, _size| serde_json::json!({"ResourceArn": arn}),
        },
    ]
}

/// The error shapes the model declares for `operation`, by short name.
///
/// This service's model gives its service shape no common error list
/// (`smithy_query(query="service_info")` reports no `common_errors`), so an operation's
/// own `errors` list is the whole set it may answer.
pub(crate) fn declared_errors(model: &serde_json::Value, operation: &str) -> Vec<String> {
    let id = format!("{NAMESPACE}#{operation}");
    model["shapes"][&id]["errors"]
        .as_array()
        .unwrap_or_else(|| panic!("the model declares no error list for {id}"))
        .iter()
        .map(|entry| {
            let target = entry["target"]
                .as_str()
                .unwrap_or_else(|| panic!("an error entry of {id} names no target shape"));
            shape_name(target).to_string()
        })
        .collect()
}

/// The short name of a shape id (`namespace#Name` -> `Name`) — the form the wire
/// `__type` ends with.
///
/// `pub(crate)` beside [`declared_errors`] so every suite asserting "only the errors this
/// operation declares" reads that set through ONE definition.
pub(crate) fn shape_name(id: &str) -> &str {
    id.split('#').next_back().unwrap_or(id)
}

/// One row of the absent-parent table: a workflow ARN, the endpoint the call is made
/// at, and why that endpoint does not hold it.
struct AbsentParent {
    label: String,
    arn: String,
    endpoint: &'static str,
}

/// Build every absent-parent row against a fresh store, and report it with the control
/// workflow the same store DOES hold.
async fn absent_parents(harness: &Harness) -> Vec<AbsentParent> {
    let mut rows: Vec<AbsentParent> = ghost_arns()
        .into_iter()
        .map(|arn| AbsentParent {
            label: format!("never created/{arn}"),
            arn,
            endpoint: HOME,
        })
        .collect();

    // A workflow whose teardown the clock has already completed is as absent as one
    // that never existed.
    let deleted = created_workflow(harness, "listedThenDeleted").await;
    started_run(harness, &deleted, None).await;
    let (status, removed) = harness
        .call("DeleteWorkflow", serde_json::json!({"WorkflowArn": deleted}))
        .await;
    assert_eq!(
        status, 200,
        "setup: expected DeleteWorkflow to succeed, got {status} {removed:?}"
    );
    tokio::time::sleep(WORKFLOW_DELETED_AFTER + Duration::from_millis(200)).await;
    rows.push(AbsentParent {
        label: format!("torn down/{deleted}"),
        arn: deleted,
        endpoint: HOME,
    });

    // A workflow another region's endpoint created is not one this endpoint holds.
    let elsewhere = created_workflow(harness, "heldByAnotherRegion").await;
    rows.push(AbsentParent {
        label: format!("another region's workflow/{elsewhere}"),
        arn: elsewhere,
        endpoint: FOREIGN,
    });

    rows
}

/// The page sizes an absent parent is asked for: none at all, and each published bound
/// of `MaxResults` (`@range(min: 1, max: 100)`, `smithy_query(query="operation",
/// operation="ListWorkflowRuns")`).
const PAGE_SIZES: &[(&str, Option<i64>)] = &[
    ("no page size", None),
    ("MaxResults at the minimum of 1", Some(1)),
    ("MaxResults at the maximum of 100", Some(100)),
];

#[tokio::test]
async fn an_operation_answers_only_the_errors_its_own_model_declares() {
    let model = smithy_model();
    let harness = Harness::new(HOME);

    // The two facts the expectations rest on, read from the model rather than written
    // out here: a child-resource listing declares no not-found error, and the
    // single-resource operations resolving the same ARN do.
    for case in operation_cases() {
        let declared = declared_errors(&model, case.operation);
        let declares_not_found = declared.iter().any(|name| name == NOT_FOUND);
        assert_eq!(
            declares_not_found,
            case.items.is_none(),
            "case {:?}: this suite's rows assume only the single-resource operations \
             declare {NOT_FOUND}; the model now declares {declared:?}",
            case.label
        );
    }

    let parents = absent_parents(&harness).await;

    for case in operation_cases() {
        let declared = declared_errors(&model, case.operation);
        let declares_not_found = declared.iter().any(|name| name == NOT_FOUND);

        for parent in &parents {
            for (size_label, size) in PAGE_SIZES {
                // A page size is only a member of the paginated listings; a
                // single-resource read is called once, plainly.
                if case.items.is_none() && size.is_some() {
                    continue;
                }
                let label = format!("{}/{}/{size_label}", case.label, parent.label);
                let body = (case.body)(&parent.arn, *size);
                let (status, answered) = harness
                    .call_in(parent.endpoint, case.operation, body)
                    .await;

                // The rule itself: whatever error shape comes back, the operation's own
                // model declares it.
                if let Some(kind) = answered["__type"].as_str() {
                    assert!(
                        declared.iter().any(|name| name == shape_name(kind)),
                        "case {label:?}: {kind:?} is outside the error set this operation \
                         declares ({declared:?}), got {status} {answered:?}"
                    );
                }

                match case.items {
                    // No not-found error to answer with, so the absent parent is the
                    // empty page the output models, and the walk ends there.
                    Some(items) => {
                        assert!(
                            !declares_not_found,
                            "case {label:?}: the listing rows are the ones declaring no \
                             {NOT_FOUND}, got {declared:?}"
                        );
                        assert_eq!(
                            status, 200,
                            "case {label:?}: expected HTTP 200, got {status} {answered:?}"
                        );
                        assert_eq!(
                            answered[items].as_array().map(Vec::len),
                            Some(0),
                            "case {label:?}: expected an empty {items} list, got {:?}",
                            answered[items]
                        );
                        assert!(
                            answered["NextToken"].is_null(),
                            "case {label:?}: an empty page ends the walk, got {:?}",
                            answered["NextToken"]
                        );
                    }
                    // The contrast: an operation that DOES declare the not-found error
                    // still refuses the same absent parent, in its own vocabulary.
                    None => {
                        assert!(
                            declares_not_found,
                            "case {label:?}: the single-resource rows are the ones declaring \
                             {NOT_FOUND}, got {declared:?}"
                        );
                        assert_eq!(
                            status, LOOKUP_MISS_STATUS,
                            "case {label:?}: expected HTTP {LOOKUP_MISS_STATUS}, got {status} \
                             {answered:?}"
                        );
                        assert_eq!(
                            answered["__type"].as_str().map(shape_name),
                            Some(NOT_FOUND),
                            "case {label:?}: expected a {NOT_FOUND}, got {:?}",
                            answered["__type"]
                        );
                    }
                }
            }
        }
    }
}

#[tokio::test]
async fn a_workflow_the_endpoint_holds_still_pages_its_children() {
    let harness = Harness::new(HOME);
    let held = created_workflow(&harness, "heldByThisEndpoint").await;
    for _ in 0..CONTROL_RUNS {
        started_run(&harness, &held, None).await;
    }

    // The control rows: an empty page is what the STORE holds, not a constant. The same
    // two listings over a workflow this endpoint does hold report its children — the
    // runs started above, and the one version the create minted.
    for (operation, items, expected) in [
        ("ListWorkflowRuns", "WorkflowRuns", CONTROL_RUNS),
        ("ListWorkflowVersions", "WorkflowVersions", 1),
    ] {
        let (status, page) = harness
            .call_in(
                HOME,
                operation,
                serde_json::json!({"WorkflowArn": held, "MaxResults": 100}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {operation:?}: expected HTTP 200, got {status} {page:?}"
        );
        assert_eq!(
            page[items].as_array().map(Vec::len),
            Some(expected),
            "case {operation:?}: expected {expected} item(s) for a workflow this endpoint \
             holds, got {:?}",
            page[items]
        );
    }

    // And listing an absent parent leaves the held workflow exactly as it was: the empty
    // pages above changed nothing a later caller sees.
    for arn in ghost_arns() {
        for operation in ["ListWorkflowRuns", "ListWorkflowVersions"] {
            let (status, answered) = harness
                .call_in(HOME, operation, serde_json::json!({"WorkflowArn": arn}))
                .await;
            assert_eq!(
                status, 200,
                "case {operation:?}/{arn}: expected HTTP 200, got {status} {answered:?}"
            );
        }
    }
    assert_eq!(
        harness.stored_run_count(&held),
        CONTROL_RUNS,
        "listing an absent workflow must not touch another workflow's runs, got {}",
        harness.stored_run_count(&held)
    );
    assert_eq!(
        harness.stored_versions(&held).len(),
        1,
        "listing an absent workflow must not touch another workflow's versions, got {:?}",
        harness.stored_versions(&held)
    );
}
