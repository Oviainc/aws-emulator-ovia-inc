// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListWorkflowVersions` pagination ACROSS a change to the
//! versions being walked.
//!
//! The rule under test: a `NextToken` this listing issued marks the POSITION the walk
//! reached, so continuing from it reports the items that remain after that position —
//! each exactly once, in append order — whatever was removed from the collection
//! meanwhile; only a token this listing did not issue is refused. `DeleteWorkflow` with
//! a `WorkflowVersion` removes exactly one version and leaves the workflow in place
//! ("The specific version of the workflow to delete. If not specified, all versions of
//! the workflow are deleted", `smithy_query(query="operation",
//! operation="DeleteWorkflow")`), so a version can disappear between two pages of
//! `ListWorkflowVersions` (`@paginated(inputToken: NextToken, outputToken: NextToken,
//! items: WorkflowVersions)`) and the walk must still finish.
//!
//! The table spans what the rule generalises over: how many versions the workflow
//! holds, the page size the walk uses, how many versions are removed mid-walk, and
//! where they sit relative to the cursor — before it, at it, after it, the latest one,
//! and every one that was left.

use crate::workflow_lifecycle::{role_arn, Harness};

/// One mid-walk removal case.
struct Case {
    /// What the row is, named in every assertion.
    label: &'static str,
    /// How many versions the workflow holds before anything is deleted.
    versions: usize,
    /// The `MaxResults` the walk pages at. Smaller than `versions`, so the first page
    /// leaves a cursor.
    page_size: i64,
    /// Which versions are deleted after the first page, as indices into the append
    /// order. The cursor sits at index `page_size - 1`.
    delete: &'static [usize],
}

/// The rows: where the removed versions sit relative to the cursor, how many there
/// are, how long the history is and how big a page is.
const CASES: &[Case] = &[
    Case {
        label: "nothing deleted",
        versions: 4,
        page_size: 2,
        delete: &[],
    },
    Case {
        label: "the cursor's own version",
        versions: 4,
        page_size: 2,
        delete: &[1],
    },
    Case {
        label: "a version before the cursor",
        versions: 4,
        page_size: 2,
        delete: &[0],
    },
    Case {
        label: "a version after the cursor",
        versions: 4,
        page_size: 2,
        delete: &[2],
    },
    Case {
        label: "the latest version",
        versions: 4,
        page_size: 2,
        delete: &[3],
    },
    Case {
        label: "the cursor's version and the one after it",
        versions: 5,
        page_size: 2,
        delete: &[1, 2],
    },
    Case {
        label: "every version left after the cursor",
        versions: 4,
        page_size: 3,
        delete: &[3],
    },
    Case {
        label: "page size one, the cursor's own version",
        versions: 6,
        page_size: 1,
        delete: &[0],
    },
    Case {
        label: "page size one, three versions spread over the history",
        versions: 6,
        page_size: 1,
        delete: &[0, 2, 4],
    },
    Case {
        label: "a long history paged three at a time, around the cursor",
        versions: 7,
        page_size: 3,
        delete: &[1, 2, 3, 5],
    },
];

/// Create a workflow holding `versions` versions and report its ARN with the
/// `WorkflowVersion` of each, in append order.
async fn history(harness: &Harness, name: &str, versions: usize) -> (String, Vec<String>) {
    assert!(
        versions >= 1,
        "setup {name:?}: a workflow always holds the version its create mints"
    );
    let (status, created) = harness
        .call(
            "CreateWorkflow",
            serde_json::json!({
                "Name": name,
                "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": "dags/v0.yaml"},
                "RoleArn": role_arn(),
                "TriggerMode": "MANUAL",
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "setup {name:?}: expected CreateWorkflow to succeed, got {status} {created:?}"
    );
    let arn = created["WorkflowArn"]
        .as_str()
        .unwrap_or_else(|| panic!("setup {name:?}: a created workflow has an ARN"))
        .to_string();

    for appended in 1..versions {
        let (status, updated) = harness
            .call(
                "UpdateWorkflow",
                serde_json::json!({
                    "WorkflowArn": arn,
                    "DefinitionS3Location": {
                        "Bucket": "defs",
                        "ObjectKey": format!("dags/v{appended}.yaml"),
                    },
                    "RoleArn": role_arn(),
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "setup {name:?}: expected UpdateWorkflow {appended} to succeed, got {status} \
             {updated:?}"
        );
    }

    let stored: Vec<String> = harness
        .stored_versions(&arn)
        .into_iter()
        .map(|(version, _)| version)
        .collect();
    assert_eq!(
        stored.len(),
        versions,
        "setup {name:?}: expected {versions} stored versions, got {stored:?}"
    );
    (arn, stored)
}

/// Call `ListWorkflowVersions` over the wire.
async fn list(
    harness: &Harness,
    arn: &str,
    max_results: i64,
    token: Option<&str>,
) -> (u16, serde_json::Value) {
    let mut body = serde_json::json!({"WorkflowArn": arn, "MaxResults": max_results});
    if let Some(token) = token {
        body["NextToken"] = serde_json::Value::String(token.to_string());
    }
    harness.call("ListWorkflowVersions", body).await
}

/// The `WorkflowVersion` of each summary on a page, in the order reported.
fn page_versions(case: &str, page: &serde_json::Value) -> Vec<String> {
    page["WorkflowVersions"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a WorkflowVersions list, got {page:?}"))
        .iter()
        .map(|item| {
            item["WorkflowVersion"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: a summary carries its WorkflowVersion, got {item:?}")
                })
                .to_string()
        })
        .collect()
}

/// Delete ONE named version of a workflow, leaving the workflow itself in place.
async fn delete_version(case: &str, harness: &Harness, arn: &str, version: &str) {
    let (status, deleted) = harness
        .call(
            "DeleteWorkflow",
            serde_json::json!({"WorkflowArn": arn, "WorkflowVersion": version}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DeleteWorkflow of {version} to succeed, got {status} {deleted:?}"
    );
}

/// Walk the rest of the listing from `token`, reporting every `WorkflowVersion` the
/// pages carried in order.
async fn continued_walk(
    case: &str,
    harness: &Harness,
    arn: &str,
    page_size: i64,
    token: String,
) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut cursor = Some(token);
    let mut pages = 0;
    while let Some(token) = cursor {
        let (status, page) = list(harness, arn, page_size, Some(&token)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a cursor this listing issued must continue the walk, got {status} \
             {page:?}"
        );
        let items = page_versions(case, &page);
        assert!(
            i64::try_from(items.len()).is_ok_and(|count| count <= page_size),
            "case {case:?}: a page carried {} items for MaxResults {page_size}",
            items.len()
        );
        seen.extend(items.iter().cloned());
        pages += 1;
        assert!(
            pages <= 32,
            "case {case:?}: pagination did not terminate, saw {seen:?}"
        );
        cursor = match page["NextToken"].as_str() {
            Some(next) => {
                assert!(
                    !items.is_empty(),
                    "case {case:?}: an empty page must not carry a NextToken, got {page:?}"
                );
                Some(next.to_string())
            }
            None => None,
        };
    }
    seen
}

#[tokio::test]
async fn a_cursor_survives_the_deletion_of_the_versions_it_walks() {
    for case in CASES {
        let harness = Harness::new("us-east-1");
        let name = format!("cursorAfterDelete{}", case.versions * 10 + case.delete.len());
        let (arn, original) = history(&harness, &name, case.versions).await;
        let label = case.label;
        let page_size = usize::try_from(case.page_size).expect("a positive page size");
        assert!(
            page_size < case.versions,
            "case {label:?}: the row must leave a second page"
        );

        // Page one, which is what hands the client the cursor.
        let (status, first) = list(&harness, &arn, case.page_size, None).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected the first page to succeed, got {status} {first:?}"
        );
        let head = page_versions(label, &first);
        assert_eq!(
            head,
            original[..page_size],
            "case {label:?}: the first page is the head of the append order, got {head:?}"
        );
        let token = first["NextToken"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {label:?}: {} versions at MaxResults {page_size} leave a cursor, got {first:?}", case.versions)
            })
            .to_string();

        // The change the rule is defined over: versions removed while the walk is in
        // flight, on either side of the cursor.
        let removed: Vec<String> = case
            .delete
            .iter()
            .map(|index| original[*index].clone())
            .collect();
        for version in &removed {
            delete_version(label, &harness, &arn, version).await;
        }

        // Deleting a named version leaves the workflow itself in place, which is what
        // makes the continuation reachable at all.
        let (status, workflow) = harness
            .call("GetWorkflow", serde_json::json!({"WorkflowArn": arn}))
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: deleting one version must leave the workflow, got {status} \
             {workflow:?}"
        );

        // The walk continues over what REMAINS after the cursor's position: the versions
        // the first page did not report, minus the deleted ones, in append order.
        let expected: Vec<String> = original[page_size..]
            .iter()
            .filter(|version| !removed.contains(version))
            .cloned()
            .collect();
        let tail = continued_walk(label, &harness, &arn, case.page_size, token).await;
        assert_eq!(
            tail, expected,
            "case {label:?}: expected the remaining versions after the cursor, got {tail:?}"
        );
        for version in &removed {
            assert!(
                !tail.contains(version),
                "case {label:?}: a deleted version must not be reported, got {tail:?}"
            );
        }
        assert!(
            tail.iter().all(|version| !head.contains(version)),
            "case {label:?}: the continuation must not repeat the first page, got {tail:?} \
             after {head:?}"
        );

        // Head plus tail is exactly what the store holds now, minus whatever was deleted
        // before it could be reported.
        let stored: Vec<String> = harness
            .stored_versions(&arn)
            .into_iter()
            .map(|(version, _)| version)
            .collect();
        let walked: Vec<String> = head
            .iter()
            .chain(tail.iter())
            .filter(|version| stored.contains(version))
            .cloned()
            .collect();
        assert_eq!(
            walked, stored,
            "case {label:?}: the walk must cover every surviving version exactly once, got \
             {walked:?}"
        );

        // And a walk started AFTER the deletions reports the same surviving set.
        let restarted = continued_walk(
            label,
            &harness,
            &arn,
            case.page_size,
            first["NextToken"].as_str().unwrap_or_default().to_string(),
        )
        .await;
        assert_eq!(
            restarted, tail,
            "case {label:?}: continuing from the same cursor twice must answer the same page, \
             got {restarted:?}"
        );
    }
}

#[tokio::test]
async fn a_cursor_survives_a_change_to_another_workflow_mid_walk() {
    let harness = Harness::new("eu-west-1");
    let (arn, original) = history(&harness, "walkedWorkflow", 5).await;
    let (other, other_versions) = history(&harness, "unrelatedWorkflow", 3).await;

    let case = "an unrelated workflow changed mid-walk";
    let (status, first) = list(&harness, &arn, 2, None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the first page to succeed, got {status} {first:?}"
    );
    let token = first["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: five versions at MaxResults 2 leave a cursor"))
        .to_string();

    // One version of the OTHER workflow goes, and then the other workflow goes
    // entirely: neither touches the history being walked.
    delete_version(case, &harness, &other, &other_versions[0]).await;
    let (status, removed) = harness
        .call("DeleteWorkflow", serde_json::json!({"WorkflowArn": other}))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DeleteWorkflow to succeed, got {status} {removed:?}"
    );

    let tail = continued_walk(case, &harness, &arn, 2, token).await;
    assert_eq!(
        tail,
        original[2..].to_vec(),
        "case {case:?}: the walk must continue over the untouched history, got {tail:?}"
    );
}
