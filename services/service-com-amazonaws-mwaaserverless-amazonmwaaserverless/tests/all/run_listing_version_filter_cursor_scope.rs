// SPDX-License-Identifier: CC0-1.0

//! What `ListWorkflowRuns` does with a pagination cursor that was issued under a
//! DIFFERENT `WorkflowVersion` filter of the same workflow.
//!
//! RULE under test: a `NextToken` is a position in the one walk it was issued for, so
//! its scope carries every selector of the request that narrows the set that walk
//! covers — for this listing the parent workflow AND the `WorkflowVersion` filter,
//! which selects which of that workflow's runs the walk reports ("If not specified,
//! runs for all versions are returned", `smithy_query(query="operation",
//! operation="ListWorkflowRuns")`, `WorkflowVersion`) — and a cursor presented under
//! another selector is refused with the modelled `ValidationException` (HTTP 400, no
//! page), instead of being applied as a position in a sequence it says nothing about,
//! which silently drops every stored run the new filter selects before that position.
//!
//! The tables span what the rule generalises over: which filter issued the cursor and
//! which filter is presented it (no filter at all, each stored version, a
//! pattern-valid version no run carries, and version values made of the very
//! characters the scope is composed from), in both directions; how many runs the
//! filter presented the cursor selects next to the one that issued it (more, as many,
//! fewer, none); where the foreign position falls inside the presented filter's set,
//! which the interleaved seeding puts in the middle of it; and the page size at each
//! published `MaxResults` bound. The control rows present each cursor back under the
//! filter that issued it, so the refusal is shown to be about the cursor's scope
//! rather than about carrying a cursor at all, and show the accepted walk covering
//! exactly that filter's runs.

use crate::workflow_lifecycle::Harness;
use crate::workflow_run_listing::{
    appended_version, created_workflow, list, paged_run_ids, started_run,
};

/// The region the workflow under test is created at.
const HOME: &str = "eu-central-1";

/// The published bounds of `MaxResults` (`@range(min: 1, max: 100)`,
/// `smithy_query(query="operation", operation="ListWorkflowRuns")`), plus a size
/// between them: the page size must not change whether a foreign cursor is refused.
const PAGE_SIZES: &[(&str, i64)] = &[
    ("MaxResults at the minimum of 1", 1),
    ("MaxResults of 2", 2),
    ("MaxResults at the maximum of 100", 100),
];

/// How many runs each stored version is given, so no two filters select the same
/// number of runs and a row cannot pass by accident of counting.
const RUNS_PER_VERSION: &[usize] = &[3, 2, 4];

/// A `WorkflowVersion` selector of the modelled form (`^.+$`, length 1..2048,
/// `smithy_query(query="operation", operation="ListWorkflowRuns")`, `WorkflowVersion`)
/// that no run carries.
fn ghost_version() -> String {
    "f".repeat(32)
}

/// Version selectors no run carries whose TEXT is built from what a cursor's scope is
/// composed of, so a cursor is shown to be bound to the filter itself rather than to a
/// string that merely reads like one: the marker an unfiltered walk is written with,
/// the marker a named one is, the separator between a position and its scope, and a
/// value spelling a whole scope suffix. Each is a legal selector, since the member's
/// pattern is `^.+$`.
const SCOPE_SHAPED_VERSIONS: &[(&str, &str)] = &[
    ("a version selector of a single dash", "-"),
    ("a version selector of a single equals sign", "="),
    ("a version selector of a single at sign", "@"),
    ("a version selector spelling a scope suffix", "/versions/-"),
];

/// One filter in the table: how a case names it, the selector a request carries (none
/// for the unfiltered walk) and the runs it selects, oldest first.
struct Filter {
    label: String,
    selector: Option<String>,
    runs: Vec<String>,
}

/// One workflow holding three versions with an INTERLEAVED run history, so one
/// filter's position always falls in the middle of another filter's set — the case in
/// which honouring a foreign cursor drops stored runs rather than merely re-serving
/// them.
async fn seeded(harness: &Harness) -> (String, Vec<Filter>) {
    let arn = created_workflow(harness, "versionFilterCursor").await;
    let (status, workflow) = harness
        .call_in(HOME, "GetWorkflow", serde_json::json!({"WorkflowArn": arn}))
        .await;
    assert_eq!(
        status, 200,
        "setup: expected GetWorkflow to succeed, got {status} {workflow:?}"
    );
    let mut versions = vec![workflow["WorkflowVersion"]
        .as_str()
        .expect("setup: a created workflow has a version")
        .to_string()];
    for index in 2..=RUNS_PER_VERSION.len() {
        versions.push(appended_version(harness, &arn, &format!("dags/v{index}.yaml")).await);
    }

    // Round-robin over the versions, so the runs of any two filters alternate.
    let mut runs: Vec<Vec<String>> = vec![Vec::new(); versions.len()];
    let mut all: Vec<String> = Vec::new();
    let deepest = RUNS_PER_VERSION.iter().copied().max().unwrap_or(0);
    for round in 0..deepest {
        for (index, version) in versions.iter().enumerate() {
            if round >= RUNS_PER_VERSION[index] {
                continue;
            }
            let run_id = started_run(harness, &arn, Some(version)).await;
            all.push(run_id.clone());
            runs[index].push(run_id);
        }
    }

    let mut filters = vec![Filter {
        label: "no filter".to_string(),
        selector: None,
        runs: all,
    }];
    for (index, version) in versions.iter().enumerate() {
        filters.push(Filter {
            label: format!("version {} of {}", index + 1, versions.len()),
            selector: Some(version.clone()),
            runs: runs[index].clone(),
        });
    }
    filters.push(Filter {
        label: "a version no run carries".to_string(),
        selector: Some(ghost_version()),
        runs: Vec::new(),
    });
    for (label, selector) in SCOPE_SHAPED_VERSIONS {
        filters.push(Filter {
            label: (*label).to_string(),
            selector: Some((*selector).to_string()),
            runs: Vec::new(),
        });
    }
    (arn, filters)
}

/// The `RunId`s one page reports, in the order it reported them.
fn run_ids(page: &serde_json::Value) -> Vec<String> {
    page["WorkflowRuns"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item["RunId"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// A cursor every filter selecting more than one run issues, at a page size that
/// leaves at least one run of that filter still to come.
async fn issued_cursors(
    harness: &Harness,
    arn: &str,
    filters: &[Filter],
) -> Vec<(usize, String, String)> {
    let mut cursors = Vec::new();
    for (index, filter) in filters.iter().enumerate() {
        for (size_label, size) in PAGE_SIZES {
            if filter.runs.len() as i64 <= *size {
                continue;
            }
            let (status, first) =
                list(harness, arn, Some(*size), None, filter.selector.as_deref()).await;
            assert_eq!(
                status, 200,
                "setup {:?}/{size_label}: expected the first page to succeed, got {status} \
                 {first:?}",
                filter.label
            );
            let token = first["NextToken"].as_str().unwrap_or_else(|| {
                panic!(
                    "setup {:?}/{size_label}: {} runs at MaxResults {size} must leave a cursor, \
                     got {first:?}",
                    filter.label,
                    filter.runs.len()
                )
            });
            cursors.push((
                index,
                format!("{} at {size_label}", filter.label),
                token.to_string(),
            ));
        }
    }
    assert!(
        cursors.len() >= 4,
        "setup: the table needs cursors from several filters to travel between, got {}",
        cursors.len()
    );
    cursors
}

#[tokio::test]
async fn a_cursor_issued_under_another_version_filter_is_refused() {
    let harness = Harness::new(HOME);
    let (arn, filters) = seeded(&harness).await;
    let stored = harness.stored_run_count(&arn);
    let cursors = issued_cursors(&harness, &arn, &filters).await;

    for (issued_by, issuer, token) in &cursors {
        for (index, target) in filters.iter().enumerate() {
            // A cursor presented under the filter it was issued under is the control
            // test below; here every OTHER filter.
            if index == *issued_by {
                continue;
            }
            for (size_label, size) in PAGE_SIZES {
                let case = format!(
                    "cursor of {issuer:?} presented under {:?} selecting {} runs/{size_label}",
                    target.label,
                    target.runs.len()
                );
                let (status, answered) = list(
                    &harness,
                    &arn,
                    Some(*size),
                    Some(token),
                    target.selector.as_deref(),
                )
                .await;
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected HTTP 400 for a cursor issued under another version \
                     filter, got {status} {answered:?}"
                );
                assert!(
                    answered["__type"]
                        .as_str()
                        .is_some_and(|kind| kind.contains("ValidationException")),
                    "case {case:?}: expected a ValidationException, got {:?}",
                    answered["__type"]
                );
                assert!(
                    answered["Message"]
                        .as_str()
                        .is_some_and(|message| message.contains("NextToken")),
                    "case {case:?}: the refusal names the member it refuses, got {:?}",
                    answered["Message"]
                );
                assert!(
                    answered["WorkflowRuns"].is_null() && answered["NextToken"].is_null(),
                    "case {case:?}: a refusal reports no page and no cursor, got {answered:?}"
                );
            }
        }
    }

    // The refusals changed nothing a later caller sees: the store still holds every
    // seeded run, and each filter still reports exactly its own runs, oldest first.
    assert_eq!(
        harness.stored_run_count(&arn),
        stored,
        "a refused cursor must not change what the store holds, got {}",
        harness.stored_run_count(&arn)
    );
    for filter in &filters {
        let (status, whole) =
            list(&harness, &arn, Some(100), None, filter.selector.as_deref()).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the whole set to list, got {status} {whole:?}",
            filter.label
        );
        assert_eq!(
            run_ids(&whole),
            filter.runs,
            "case {:?}: a refused cursor must not change what the filter selects, got {:?}",
            filter.label,
            run_ids(&whole)
        );
    }
}

#[tokio::test]
async fn a_cursor_presented_under_the_filter_that_issued_it_continues_that_walk() {
    let harness = Harness::new(HOME);
    let (arn, filters) = seeded(&harness).await;

    for filter in &filters {
        for (size_label, size) in PAGE_SIZES {
            let case = format!("{:?}/{size_label}", filter.label);
            let seen =
                paged_run_ids(&harness, &arn, *size, filter.selector.as_deref(), &case).await;
            assert_eq!(
                seen, filter.runs,
                "case {case:?}: paging through its own cursors must cover exactly the runs that \
                 filter selects, oldest first, got {seen:?}"
            );
        }
    }
}
