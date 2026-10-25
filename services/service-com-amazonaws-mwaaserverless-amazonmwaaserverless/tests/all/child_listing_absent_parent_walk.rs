// SPDX-License-Identifier: CC0-1.0

//! What a paginated child-resource listing answers for a parent workflow the endpoint
//! does not hold — at EVERY point of the walk, and for every form of ARN the model
//! admits.
//!
//! RULE under test: an operation answers only the error shapes its OWN modelled error
//! set declares, so a child-resource listing — whose set declares no not-found error —
//! reports a parent the endpoint does not hold as the empty page its output models,
//! whether the parent is named on the first page or on a page resumed through a cursor
//! the listing itself issued, and whatever partition, region, account or resource name
//! the ARN carries. The refusals the set DOES declare are unaffected: an ARN outside the
//! member's modelled constraint is still the declared `ValidationException`.
//!
//! The sibling suite `child_listing_error_set` spans the operations and the reasons a
//! parent is absent on a FIRST page. These tables span the two dimensions it does not:
//! every partition prefix the `WorkflowArn` pattern admits (with the shortest and the
//! longest resource name it allows), and the position in the walk — a parent torn down
//! between two pages, where the cursor the listing minted is still presented back to it.
//!
//! `ListTaskInstances` is the third child-resource listing and declares no not-found
//! error either; the suite `task_instance_listing` carries the rule onto it, including
//! the cursor it issues for one run's walk.

use std::time::Duration;

use service_com_amazonaws_mwaaserverless_amazonmwaaserverless::state::WORKFLOW_DELETED_AFTER;

use crate::child_listing_error_set::{declared_errors, shape_name};
use crate::validation_errors::smithy_model;
use crate::workflow_lifecycle::Harness;
use crate::workflow_run_listing::{appended_version, created_workflow, started_run};

/// The error shape no child-resource listing declares.
const NOT_FOUND: &str = "ResourceNotFoundException";

/// The refusal every operation of this service DOES declare for a member outside its
/// modelled constraint.
const VALIDATION: &str = "ValidationException";

/// The endpoint every call below is signed for.
const HOME: &str = "us-east-1";

/// How many children the mid-walk rows give their workflow, so the walk at a page size
/// of one has a page left to resume onto.
const CHILDREN: usize = 3;

/// One paginated child-resource listing: its operation name and the `@paginated`
/// `items` member it reports a page in
/// (`smithy_query(query="operation", operation="ListWorkflowRuns,ListWorkflowVersions")`).
const LISTINGS: &[(&str, &str)] = &[
    ("ListWorkflowRuns", "WorkflowRuns"),
    ("ListWorkflowVersions", "WorkflowVersions"),
];

/// Page sizes the tables ask for: none at all, and each published bound of `MaxResults`
/// (`@range(min: 1, max: 100)`, `smithy_query(query="operation",
/// operation="ListWorkflowVersions")`).
const PAGE_SIZES: &[(&str, Option<i64>)] = &[
    ("no page size", None),
    ("MaxResults at the minimum of 1", Some(1)),
    ("MaxResults at the maximum of 100", Some(100)),
];

/// Call one of the child-resource listings over the wire.
///
/// Local to this suite because the rows drive BOTH listings through one code path: the
/// per-operation helpers (`workflow_run_listing::list`,
/// `workflow_version_listing::list`) each take the members of their own operation.
async fn list(
    harness: &Harness,
    operation: &str,
    arn: &str,
    max_results: Option<i64>,
    token: Option<&str>,
) -> (u16, serde_json::Value) {
    let mut body = serde_json::json!({"WorkflowArn": arn});
    if let Some(size) = max_results {
        body["MaxResults"] = serde_json::Value::from(size);
    }
    if let Some(token) = token {
        body["NextToken"] = serde_json::Value::String(token.to_string());
    }
    harness.call(operation, body).await
}

/// Assert `answered` is the empty page the listing's output models, and that nothing
/// about it is outside `declared`.
fn assert_empty_page(
    label: &str,
    items: &str,
    declared: &[String],
    status: u16,
    answered: &serde_json::Value,
) {
    if let Some(kind) = answered["__type"].as_str() {
        assert!(
            declared.iter().any(|name| name == shape_name(kind)),
            "case {label:?}: {kind:?} is outside the error set this operation declares \
             ({declared:?}), got {status} {answered:?}"
        );
    }
    assert_eq!(
        status, 200,
        "case {label:?}: expected HTTP 200 for a parent this endpoint does not hold, got \
         {status} {answered:?}"
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

/// A `WorkflowArn` no store ever held, assembled from the parts the modelled pattern
/// names.
fn ghost_arn(partition: &str, region: &str, account: &str, name_and_suffix: &str) -> String {
    format!("arn:{partition}:airflow-serverless:{region}:{account}:workflow/{name_and_suffix}")
}

/// The longest resource name the pattern allows: its name segment is
/// `[a-zA-Z0-9][a-zA-Z0-9\.\-_]{0,254}`, so at most 255 characters, followed by `-` and
/// the 10-character suffix.
fn longest_name() -> String {
    format!("{}-0123456789", "n".repeat(255))
}

/// One pattern-valid ARN per partition prefix the modelled `WorkflowArn` admits.
///
/// Separate from the shared `missing_resource_lookups::ghost_arns` because that set is
/// the one every suite needing *an* absent ARN draws from and is deliberately all in the
/// `aws` partition; the dimension these rows exist for is the PARTITION prefix, which no
/// other suite varies.
///
/// (`^arn:aws(?:-(?:cn|us-gov|iso|iso-b|iso-e|iso-f))?:airflow-serverless:([a-z]{2}-[a-z]+-[0-9]{1}):([0-9]{12}):workflow/([a-zA-Z0-9][a-zA-Z0-9\.\-_]{0,254}-[a-zA-Z0-9]{10})$`,
/// `smithy_query(query="shape", shape="WorkflowArn")`), with a region the pattern's
/// region segment admits for that partition, and the account and resource-name forms
/// varied across the rows — including the shortest name segment the pattern allows (one
/// character, plus the suffix) and the longest (255 characters, plus the suffix).
///
/// No workflow of any of these ARNs was ever created, so each is a parent the endpoint
/// does not hold.
fn ghost_arns_across_partitions() -> Vec<String> {
    vec![
        ghost_arn("aws", "us-east-1", "123456789012", "ghost-abcdefghij"),
        ghost_arn("aws-cn", "cn-north-1", "000000000000", "a-0123456789"),
        ghost_arn("aws-us-gov", "us-west-1", "999999999999", &longest_name()),
        ghost_arn("aws-iso", "us-east-1", "210987654321", "absent.wf_name-ABCDEFGHIJ"),
        ghost_arn("aws-iso-b", "eu-west-3", "000000000001", "G-9999999999"),
        ghost_arn("aws-iso-e", "ap-south-1", "555555555555", "absent--name-aB9cD8eF7g"),
        ghost_arn("aws-iso-f", "sa-east-1", "111111111111", &longest_name()),
    ]
}

/// ARNs outside the modelled `WorkflowArn` constraint: the refusal a listing's own error
/// set DOES declare, so an empty page is shown to be what the store holds rather than a
/// blanket success.
fn malformed_arns() -> Vec<(&'static str, String)> {
    vec![
        ("not an ARN at all", "ghost".to_string()),
        (
            "another service's ARN",
            "arn:aws:s3:::definitions/ghost-abcdefghij".to_string(),
        ),
        (
            "a partition the pattern does not admit",
            ghost_arn("aws-moon", "us-east-1", "123456789012", "ghost-abcdefghij"),
        ),
        (
            "an account that is not twelve digits",
            ghost_arn("aws", "us-east-1", "12345", "ghost-abcdefghij"),
        ),
        (
            "a resource name without the ten-character suffix",
            ghost_arn("aws", "us-east-1", "123456789012", "ghost"),
        ),
        (
            "a name segment one character past the 255 the pattern allows",
            ghost_arn(
                "aws",
                "us-east-1",
                "123456789012",
                &format!("{}-0123456789", "n".repeat(256)),
            ),
        ),
    ]
}

#[tokio::test]
async fn an_absent_parent_is_an_empty_page_for_every_arn_form_the_model_admits() {
    let model = smithy_model();
    let harness = Harness::new(HOME);

    // The control: a workflow this endpoint DOES hold, so the empty pages below are what
    // the store holds and not a constant.
    let held = created_workflow(&harness, "heldWhileGhostsAreListed").await;
    started_run(&harness, &held, None).await;

    for (operation, items) in LISTINGS {
        let declared = declared_errors(&model, operation);
        assert!(
            !declared.iter().any(|name| name == NOT_FOUND),
            "case {operation:?}: these rows are the listings declaring no {NOT_FOUND}; the \
             model now declares {declared:?}"
        );
        assert!(
            declared.iter().any(|name| name == VALIDATION),
            "case {operation:?}: the malformed rows need the declared {VALIDATION}; the \
             model now declares {declared:?}"
        );

        for arn in ghost_arns_across_partitions() {
            for (size_label, size) in PAGE_SIZES {
                let label = format!("{operation}/{arn}/{size_label}");
                let (status, answered) = list(&harness, operation, &arn, *size, None).await;
                assert_empty_page(&label, items, &declared, status, &answered);
            }
        }

        // An ARN the modelled constraint rejects is refused with the error the set does
        // declare, at the status the model gives it.
        for (case, arn) in malformed_arns() {
            let label = format!("{operation}/{case}");
            let (status, answered) = list(&harness, operation, &arn, None, None).await;
            assert_eq!(
                status, 400,
                "case {label:?}: expected HTTP 400 for an ARN outside the modelled \
                 constraint, got {status} {answered:?}"
            );
            assert_eq!(
                answered["__type"].as_str().map(shape_name),
                Some(VALIDATION),
                "case {label:?}: expected a {VALIDATION}, got {:?}",
                answered["__type"]
            );
        }

        // And the held workflow still reports its child, unchanged by any of it.
        let (status, page) = list(&harness, operation, &held, Some(100), None).await;
        assert_eq!(
            status, 200,
            "case {operation:?}/held: expected HTTP 200, got {status} {page:?}"
        );
        assert_eq!(
            page[items].as_array().map(Vec::len),
            Some(1),
            "case {operation:?}/held: expected the one child this workflow holds, got {:?}",
            page[items]
        );
    }
}

#[tokio::test]
async fn a_parent_torn_down_between_two_pages_ends_the_walk_with_an_empty_page() {
    let model = smithy_model();

    for (operation, items) in LISTINGS {
        let declared = declared_errors(&model, operation);
        // A fresh store per listing, so the teardown of one row's workflow cannot be
        // mistaken for the other's.
        let harness = Harness::new(HOME);
        let arn = created_workflow(&harness, "tornDownMidWalk").await;

        // CHILDREN children of the kind this listing pages: a create already mints the
        // first version, and each update mints one more; each run is started explicitly.
        match *operation {
            "ListWorkflowVersions" => {
                for index in 1..CHILDREN {
                    appended_version(&harness, &arn, &format!("dags/v{index}.yaml")).await;
                }
            }
            _ => {
                for _ in 0..CHILDREN {
                    started_run(&harness, &arn, None).await;
                }
            }
        }

        // The first page, at the smallest page size the range allows, so a cursor is
        // issued with pages still to come.
        let (status, first) = list(&harness, operation, &arn, Some(1), None).await;
        assert_eq!(
            status, 200,
            "case {operation:?}/first page: expected HTTP 200, got {status} {first:?}"
        );
        let token = first["NextToken"]
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "setup {operation:?}: {CHILDREN} children at MaxResults 1 must leave a \
                     cursor, got {first:?}"
                );
            })
            .to_string();

        // The cursor really does resume the walk while the parent is still held — so the
        // empty page below is the absent parent speaking, not a rejected token.
        let (status, second) = list(&harness, operation, &arn, Some(1), Some(&token)).await;
        assert_eq!(
            status, 200,
            "case {operation:?}/second page: expected HTTP 200, got {status} {second:?}"
        );
        assert_eq!(
            second[items].as_array().map(Vec::len),
            Some(1),
            "case {operation:?}/second page: expected the next child, got {:?}",
            second[items]
        );

        // Now the parent goes away between pages, teardown included.
        let (status, removed) = harness
            .call("DeleteWorkflow", serde_json::json!({"WorkflowArn": arn}))
            .await;
        assert_eq!(
            status, 200,
            "setup {operation:?}: expected DeleteWorkflow to succeed, got {status} {removed:?}"
        );
        tokio::time::sleep(WORKFLOW_DELETED_AFTER + Duration::from_millis(200)).await;

        // Resumed through the cursor it issued, and started afresh: both are the empty
        // page, at every page size.
        for (size_label, size) in PAGE_SIZES {
            for (position, cursor) in [("resumed", Some(token.as_str())), ("restarted", None)] {
                let label = format!("{operation}/torn down mid-walk/{position}/{size_label}");
                let (status, answered) = list(&harness, operation, &arn, *size, cursor).await;
                assert_empty_page(&label, items, &declared, status, &answered);
            }
        }

        // The listing resurrected nothing: the store still holds no version of the
        // torn-down workflow and no run of it.
        assert!(
            harness.stored_versions(&arn).is_empty(),
            "case {operation:?}: listing a torn-down parent must not resurrect its \
             versions, got {:?}",
            harness.stored_versions(&arn)
        );
        assert_eq!(
            harness.stored_run_count(&arn),
            0,
            "case {operation:?}: listing a torn-down parent must not resurrect its runs, \
             got {}",
            harness.stored_run_count(&arn)
        );
    }
}
