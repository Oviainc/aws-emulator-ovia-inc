// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListWorkflowRuns`.
//!
//! The rule under test: `ListWorkflowRuns` reports one `WorkflowRunSummary` per run the
//! STORE holds under the workflow its `WorkflowArn` names — in `RunId` order, at most
//! `MaxResults` per page, continued through the opaque `NextToken` this service itself
//! issued, and narrowed to a single `WorkflowVersion` when the request names one, since
//! "If not specified, runs for all versions are returned"
//! (`smithy_query(query="operation", operation="ListWorkflowRuns")`, `WorkflowVersion`).
//! Each summary carries exactly the stored run's `RunId`, `WorkflowArn`,
//! `WorkflowVersion`, `RunType` and `RunDetailSummary` — the same status and instants
//! `GetWorkflowRun` serves from that record — and a workflow the store does not hold is
//! the service's lookup miss rather than an empty page. The listing persists nothing.
//!
//! The tables span what the rule generalises over: the number of runs, the page size
//! (including both published `MaxResults` bounds), which workflow version a run carries,
//! the status a run is in, and the form of a `NextToken`. Every run that must report a
//! FIXED status is seeded with its next clock-owed change far in the future, so a status
//! cannot move between two calls of one assertion.

use std::time::{Duration, SystemTime};

use service_com_amazonaws_mwaaserverless_amazonmwaaserverless::state::{
    RUN_STATUS_FAILED, RUN_STATUS_QUEUED, RUN_STATUS_STARTING, RUN_TYPE_ON_DEMAND,
    WORKFLOW_DELETED_AFTER, WORKFLOW_STATUS_DELETING,
};

use crate::missing_resource_lookups::{ghost_arns, LOOKUP_MISS_STATUS, WORKFLOW_MISSING};
use crate::run_lifecycle_progression::with_run;
use crate::workflow_lifecycle::{role_arn, Harness};

/// The wire error code a lookup miss carries.
const LOOKUP_MISS_TYPE: &str = "ResourceNotFoundException";

/// The `ResourceType` a refusal reports for an absent workflow.
const WORKFLOW_RESOURCE_TYPE: &str = "workflow";

/// A `WorkflowVersion` of the modelled form (`^[a-z0-9]{32}$`) that no run here carries.
fn ghost_version() -> String {
    "f".repeat(32)
}

/// Create a workflow and report its ARN.
async fn created_workflow(harness: &Harness, name: &str) -> String {
    let (status, created) = harness
        .call(
            "CreateWorkflow",
            serde_json::json!({
                "Name": name,
                "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": "dags/listing.yaml"},
                "RoleArn": role_arn(),
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "setup: expected CreateWorkflow to succeed, got {status} {created:?}"
    );
    created["WorkflowArn"]
        .as_str()
        .expect("setup: a created workflow has an ARN")
        .to_string()
}

/// Append a version to a workflow and report the new `WorkflowVersion`.
async fn appended_version(harness: &Harness, arn: &str, object_key: &str) -> String {
    let (status, updated) = harness
        .call(
            "UpdateWorkflow",
            serde_json::json!({
                "WorkflowArn": arn,
                "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": object_key},
                "RoleArn": role_arn(),
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "setup: expected UpdateWorkflow to succeed, got {status} {updated:?}"
    );
    updated["WorkflowVersion"]
        .as_str()
        .expect("setup: an update mints a WorkflowVersion")
        .to_string()
}

/// Start one run, optionally on a named version, and report its `RunId`.
async fn started_run(harness: &Harness, arn: &str, version: Option<&str>) -> String {
    let mut body = serde_json::json!({"WorkflowArn": arn});
    if let Some(version) = version {
        body["WorkflowVersion"] = serde_json::Value::String(version.to_string());
    }
    let (status, started) = harness.call("StartWorkflowRun", body).await;
    assert_eq!(
        status, 200,
        "setup: expected StartWorkflowRun to succeed, got {status} {started:?}"
    );
    started["RunId"]
        .as_str()
        .expect("setup: a started run has a RunId")
        .to_string()
}

/// Pin a stored run to `status` with no further change due, so every read of it answers
/// the same thing however long the test takes.
fn pinned(harness: &Harness, arn: &str, run_id: &str, status: &str, age_millis: u64) {
    let now = SystemTime::now();
    let started_on = now - Duration::from_millis(age_millis);
    with_run(harness, arn, run_id, |run| {
        run.status = status.to_string();
        run.created_at = started_on;
        run.started_on = started_on;
        run.modified_at = started_on;
        // A terminal run has completed; one still working has not.
        if status == RUN_STATUS_FAILED {
            run.completed_on = Some(now);
            run.duration = Some(1);
        } else {
            run.completed_on = None;
            run.duration = None;
        }
        // Far enough out that no transition is owed for the rest of the test.
        run.next_status_at = now + Duration::from_secs(3_600);
    });
}

/// Call `ListWorkflowRuns` and return its status and body.
async fn list(
    harness: &Harness,
    arn: &str,
    max_results: Option<i64>,
    token: Option<&str>,
    version: Option<&str>,
) -> (u16, serde_json::Value) {
    let mut body = serde_json::json!({"WorkflowArn": arn});
    if let Some(size) = max_results {
        body["MaxResults"] = serde_json::Value::from(size);
    }
    if let Some(token) = token {
        body["NextToken"] = serde_json::Value::String(token.to_string());
    }
    if let Some(version) = version {
        body["WorkflowVersion"] = serde_json::Value::String(version.to_string());
    }
    harness.call("ListWorkflowRuns", body).await
}

/// Page through the whole listing at `page_size` and report every `RunId` in the order
/// the pages reported them.
async fn paged_run_ids(
    harness: &Harness,
    arn: &str,
    page_size: i64,
    version: Option<&str>,
    case: &str,
) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0;
    loop {
        let (status, page) = list(harness, arn, Some(page_size), token.as_deref(), version).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListWorkflowRuns to succeed, got {status} {page:?}"
        );
        let items = page["WorkflowRuns"]
            .as_array()
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected a WorkflowRuns list, got {page:?}");
            })
            .clone();
        assert!(
            i64::try_from(items.len()).is_ok_and(|count| count <= page_size),
            "case {case:?}: a page carried {} items for MaxResults {page_size}",
            items.len()
        );
        for item in &items {
            seen.push(
                item["RunId"]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: a summary must carry its RunId, got {item:?}");
                    })
                    .to_string(),
            );
        }
        pages += 1;
        assert!(
            pages <= 32,
            "case {case:?}: pagination did not terminate, saw {seen:?}"
        );
        match page["NextToken"].as_str() {
            Some(next) => {
                assert!(
                    !items.is_empty(),
                    "case {case:?}: an empty page must not carry a NextToken, got {page:?}"
                );
                token = Some(next.to_string());
            }
            None => break,
        }
    }
    seen
}

#[tokio::test]
async fn every_persisted_run_is_listed_exactly_once_across_pages_in_run_id_order() {
    let harness = Harness::new("us-east-1");
    let arn = created_workflow(&harness, "pagedRuns").await;

    // The statuses the rows cycle through, so a page mixes working and terminal runs.
    let statuses = [RUN_STATUS_STARTING, RUN_STATUS_QUEUED, RUN_STATUS_FAILED];

    // Seven runs, so the page sizes below divide the set evenly and unevenly.
    let mut expected: Vec<String> = Vec::new();
    for index in 0..7_u64 {
        let run_id = started_run(&harness, &arn, None).await;
        pinned(
            &harness,
            &arn,
            &run_id,
            statuses[usize::try_from(index).expect("a small index") % statuses.len()],
            1_000 + index * 250,
        );
        expected.push(run_id);
    }
    expected.sort();

    // `MaxResults` is modelled `@range(min: 1, max: 100)`: both published bounds plus
    // sizes smaller than, equal to and larger than the set.
    for page_size in [1_i64, 2, 3, 6, 7, 8, 100] {
        let case = format!("MaxResults={page_size}");
        let seen = paged_run_ids(&harness, &arn, page_size, None, &case).await;
        assert_eq!(
            seen, expected,
            "case {case:?}: paging must yield every run exactly once, in RunId order, got \
             {seen:?}"
        );
    }

    // The modelled default `MaxResults` of 20 covers the whole set in one page, and a
    // complete page carries no cursor.
    let (status, defaulted) = list(&harness, &arn, None, None, None).await;
    assert_eq!(
        status, 200,
        "case default MaxResults: expected ListWorkflowRuns to succeed, got {status} \
         {defaulted:?}"
    );
    assert_eq!(
        defaulted["WorkflowRuns"].as_array().map(Vec::len),
        Some(expected.len()),
        "case default MaxResults: the modelled default of 20 must cover all {}, got {:?}",
        expected.len(),
        defaulted["WorkflowRuns"]
    );
    assert!(
        defaulted["NextToken"].is_null(),
        "case default MaxResults: a complete page must carry no NextToken, got {:?}",
        defaulted["NextToken"]
    );

    // A workflow that holds no run is an EMPTY page, not a refusal and not an absent
    // member: the listing's own resource exists, it just has nothing in it.
    let empty_arn = created_workflow(&harness, "noRunsAtAll").await;
    for page_size in [None, Some(1), Some(100)] {
        let (status, empty) = list(&harness, &empty_arn, page_size, None, None).await;
        assert_eq!(
            status, 200,
            "case empty/{page_size:?}: expected ListWorkflowRuns to succeed, got {status} \
             {empty:?}"
        );
        assert_eq!(
            empty["WorkflowRuns"].as_array().map(Vec::len),
            Some(0),
            "case empty/{page_size:?}: a workflow with no run must report an empty list, got \
             {:?}",
            empty["WorkflowRuns"]
        );
        assert!(
            empty["NextToken"].is_null(),
            "case empty/{page_size:?}: an empty page carries no NextToken, got {:?}",
            empty["NextToken"]
        );
    }

    // The listing is a read: it persists nothing and leaves the run store as it was.
    assert_eq!(
        harness.stored_run_count(&arn),
        expected.len(),
        "a listing must not create or remove a run, got {}",
        harness.stored_run_count(&arn)
    );
    assert_eq!(
        harness.stored_run_count(&empty_arn),
        0,
        "listing a workflow with no run must not create one, got {}",
        harness.stored_run_count(&empty_arn)
    );
}

#[tokio::test]
async fn a_listed_summary_reports_the_same_run_get_workflow_run_does() {
    let harness = Harness::new("eu-west-1");
    let arn = created_workflow(&harness, "summaryFidelity").await;

    // The rows differ along the status the run is in — and so along whether it has
    // completed, which is what decides if `EndedAt` is reported: `EndedAt` "is null if
    // the run is not complete" (`smithy_query(query="shape", shape="RunDetailSummary")`).
    let rows: Vec<(&str, u64)> = vec![
        (RUN_STATUS_STARTING, 0),
        (RUN_STATUS_QUEUED, 1),
        (RUN_STATUS_FAILED, 3_600_000),
    ];

    let mut run_ids: Vec<(String, &str)> = Vec::new();
    for (status, age_millis) in &rows {
        let run_id = started_run(&harness, &arn, None).await;
        pinned(&harness, &arn, &run_id, status, *age_millis);
        run_ids.push((run_id, status));
    }

    let (status, listed) = list(&harness, &arn, Some(100), None, None).await;
    assert_eq!(
        status, 200,
        "expected ListWorkflowRuns to succeed, got {status} {listed:?}"
    );
    let summaries = listed["WorkflowRuns"]
        .as_array()
        .expect("the listing reports a WorkflowRuns list")
        .clone();
    assert_eq!(
        summaries.len(),
        run_ids.len(),
        "expected one summary per started run, got {summaries:?}"
    );

    for (run_id, expected_status) in &run_ids {
        let case = format!("{expected_status}/{run_id}");
        let summary = summaries
            .iter()
            .find(|item| item["RunId"].as_str() == Some(run_id.as_str()))
            .unwrap_or_else(|| {
                panic!("case {case:?}: the listing must report the started run, got {summaries:?}");
            });

        let (status, got) = harness
            .call(
                "GetWorkflowRun",
                serde_json::json!({"WorkflowArn": arn, "RunId": run_id}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetWorkflowRun to succeed, got {status} {got:?}"
        );

        // The identity members come from the same stored record, so the summary and the
        // detailed read cannot disagree on any of them.
        for member in ["RunId", "WorkflowArn", "WorkflowVersion", "RunType"] {
            assert_eq!(
                summary[member], got[member],
                "case {case:?}: expected the summary's {member} to match GetWorkflowRun's \
                 {:?}, got {:?}",
                got[member], summary[member]
            );
        }
        assert_eq!(
            summary["RunType"].as_str(),
            Some(RUN_TYPE_ON_DEMAND),
            "case {case:?}: a run a caller started is ON_DEMAND, got {:?}",
            summary["RunType"]
        );
        assert_eq!(
            summary["WorkflowArn"].as_str(),
            Some(arn.as_str()),
            "case {case:?}: a summary reports the workflow that holds the run, got {:?}",
            summary["WorkflowArn"]
        );

        let detail = &summary["RunDetailSummary"];
        assert_eq!(
            detail["Status"].as_str(),
            Some(*expected_status),
            "case {case:?}: expected the stored status, got {:?}",
            detail["Status"]
        );
        assert_eq!(
            detail["Status"], got["RunDetail"]["RunState"],
            "case {case:?}: the summary's Status is the run's state, got {:?} against {:?}",
            detail["Status"], got["RunDetail"]["RunState"]
        );
        // The three instants, each against the member `GetWorkflowRun` reports it as.
        for (summary_member, detail_member) in [
            ("CreatedOn", "CreatedAt"),
            ("StartedAt", "StartedOn"),
            ("EndedAt", "CompletedOn"),
        ] {
            assert_eq!(
                detail[summary_member], got["RunDetail"][detail_member],
                "case {case:?}: expected {summary_member} to match GetWorkflowRun's \
                 {detail_member} {:?}, got {:?}",
                got["RunDetail"][detail_member], detail[summary_member]
            );
        }
        let complete = *expected_status == RUN_STATUS_FAILED;
        assert_eq!(
            detail["EndedAt"].is_null(),
            !complete,
            "case {case:?}: EndedAt is reported exactly when the run is complete, got {:?}",
            detail["EndedAt"]
        );
    }

    // A read changes nothing a later caller sees: the same listing answers identically.
    let (status, again) = list(&harness, &arn, Some(100), None, None).await;
    assert_eq!(
        status, 200,
        "case repeated listing: expected ListWorkflowRuns to succeed, got {status} {again:?}"
    );
    assert_eq!(
        again, listed,
        "case repeated listing: a read must not change what a later caller sees, got \
         {again:?} after {listed:?}"
    );
}

#[tokio::test]
async fn the_workflow_version_filter_narrows_the_listing_to_the_runs_that_carry_it() {
    let harness = Harness::new("ap-south-1");
    let arn = created_workflow(&harness, "versionFilter").await;

    // Three versions of one workflow, and a different number of runs on each, so a row
    // cannot pass by accident of counting.
    let (status, workflow) = harness
        .call("GetWorkflow", serde_json::json!({"WorkflowArn": arn}))
        .await;
    assert_eq!(
        status, 200,
        "setup: expected GetWorkflow to succeed, got {status} {workflow:?}"
    );
    let first = workflow["WorkflowVersion"]
        .as_str()
        .expect("setup: a created workflow has a version")
        .to_string();
    let second = appended_version(&harness, &arn, "dags/v2.yaml").await;
    let third = appended_version(&harness, &arn, "dags/v3.yaml").await;

    let mut by_version: Vec<(String, Vec<String>)> = Vec::new();
    for (version, count) in [(&first, 2_usize), (&second, 1), (&third, 3)] {
        let mut ids = Vec::new();
        for _ in 0..count {
            ids.push(started_run(&harness, &arn, Some(version)).await);
        }
        ids.sort();
        by_version.push((version.clone(), ids));
    }
    let mut all: Vec<String> = by_version
        .iter()
        .flat_map(|(_, ids)| ids.iter().cloned())
        .collect();
    all.sort();

    // The rows: no filter (every version), each stored version, and a pattern-valid
    // version no run carries.
    let mut rows: Vec<(String, Option<String>, Vec<String>)> =
        vec![("no filter".to_string(), None, all.clone())];
    for (version, ids) in &by_version {
        rows.push((
            format!("version {version}"),
            Some(version.clone()),
            ids.clone(),
        ));
    }
    rows.push((
        "a version no run carries".to_string(),
        Some(ghost_version()),
        Vec::new(),
    ));

    for (case, version, expected) in rows {
        // One page large enough for the whole set, and a page size of one, so the filter
        // is shown to hold across the cursor as well as within a page.
        for page_size in [100_i64, 1] {
            let label = format!("{case} at MaxResults={page_size}");
            let seen =
                paged_run_ids(&harness, &arn, page_size, version.as_deref(), &label).await;
            assert_eq!(
                seen, expected,
                "case {label:?}: expected exactly the runs carrying that version, in RunId \
                 order, got {seen:?}"
            );
        }

        // Every summary a filtered listing reports really carries the version asked for.
        let (status, page) = list(&harness, &arn, Some(100), None, version.as_deref()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListWorkflowRuns to succeed, got {status} {page:?}"
        );
        if let Some(version) = version.as_deref() {
            for item in page["WorkflowRuns"]
                .as_array()
                .expect("the listing reports a WorkflowRuns list")
            {
                assert_eq!(
                    item["WorkflowVersion"].as_str(),
                    Some(version),
                    "case {case:?}: a filtered listing reports only runs of that version, \
                     got {item:?}"
                );
            }
        }
    }

    // Filtering persists nothing: every run is still there to be listed unfiltered.
    assert_eq!(
        harness.stored_run_count(&arn),
        all.len(),
        "a filtered listing must not remove a run, got {}",
        harness.stored_run_count(&arn)
    );
}

#[tokio::test]
async fn a_workflow_the_store_does_not_hold_is_the_services_lookup_miss() {
    let harness = Harness::new("us-west-2");

    // A workflow whose teardown has FINISHED is as absent as one that never existed. Its
    // `DELETING` span is observable first: while the record is still there, so are its
    // runs, and only once the clock has removed it does the listing refuse.
    let deleted = created_workflow(&harness, "listedThenDeleted").await;
    let run_id = started_run(&harness, &deleted, None).await;
    let (status, removed) = harness
        .call("DeleteWorkflow", serde_json::json!({"WorkflowArn": deleted}))
        .await;
    assert_eq!(
        status, 200,
        "setup: expected DeleteWorkflow to succeed, got {status} {removed:?}"
    );
    let (status, deleting) = list(&harness, &deleted, Some(100), None, None).await;
    assert_eq!(
        status, 200,
        "case mid-teardown: a workflow still in {WORKFLOW_STATUS_DELETING} holds its runs, \
         got {status} {deleting:?}"
    );
    assert_eq!(
        deleting["WorkflowRuns"]
            .as_array()
            .and_then(|items| items.first())
            .map(|item| item["RunId"].clone()),
        Some(serde_json::Value::String(run_id.clone())),
        "case mid-teardown: expected the run {run_id:?} to be listed until the teardown \
         completes, got {:?}",
        deleting["WorkflowRuns"]
    );
    // The removal the delete made due lands on its own, without another delete.
    tokio::time::sleep(WORKFLOW_DELETED_AFTER + Duration::from_millis(200)).await;

    let mut arns = ghost_arns();
    arns.push(deleted.clone());

    for arn in &arns {
        // The refusal does not depend on the other members: with and without a page
        // size and a version filter.
        for (case, size, version) in [
            ("plain", None, None),
            ("with a page size", Some(1_i64), None),
            ("with a version filter", Some(100), Some(ghost_version())),
        ] {
            let label = format!("{case}/{arn}");
            let (status, answered) = list(&harness, arn, size, None, version.as_deref()).await;
            assert_eq!(
                status, LOOKUP_MISS_STATUS,
                "case {label:?}: expected HTTP {LOOKUP_MISS_STATUS}, got {status} {answered:?}"
            );
            assert!(
                answered["__type"]
                    .as_str()
                    .is_some_and(|kind| kind.contains(LOOKUP_MISS_TYPE)),
                "case {label:?}: expected a {LOOKUP_MISS_TYPE}, got {:?}",
                answered["__type"]
            );
            assert_eq!(
                answered["Message"].as_str(),
                Some(WORKFLOW_MISSING),
                "case {label:?}: expected the entity-level message for an absent workflow, \
                 got {:?}",
                answered["Message"]
            );
            assert_eq!(
                answered["ResourceId"].as_str(),
                Some(arn.as_str()),
                "case {label:?}: ResourceId must name what was looked up, got {:?}",
                answered["ResourceId"]
            );
            assert_eq!(
                answered["ResourceType"].as_str(),
                Some(WORKFLOW_RESOURCE_TYPE),
                "case {label:?}: ResourceType must name the entity, got {:?}",
                answered["ResourceType"]
            );
            assert!(
                answered["WorkflowRuns"].is_null(),
                "case {label:?}: a refusal reports no page, got {:?}",
                answered["WorkflowRuns"]
            );
        }
    }

    // A refused listing creates nothing: the torn-down workflow stays gone, and the run
    // it held went with it.
    assert_eq!(
        harness.stored_run_count(&deleted),
        0,
        "a refused listing must not resurrect the run {run_id:?}, got {}",
        harness.stored_run_count(&deleted)
    );
}

#[tokio::test]
async fn a_pagination_cursor_this_listing_did_not_issue_is_refused() {
    let harness = Harness::new("eu-central-1");
    let arn = created_workflow(&harness, "cursorProbe").await;
    let other = created_workflow(&harness, "cursorProbeOther").await;
    for _ in 0..3 {
        started_run(&harness, &arn, None).await;
    }

    // A cursor issued for ANOTHER paginated list of this service: `ListWorkflows` mints
    // one of the same wire form, and it must not be honoured here.
    let (status, workflows) = harness
        .call("ListWorkflows", serde_json::json!({"MaxResults": 1}))
        .await;
    assert_eq!(
        status, 200,
        "setup: expected ListWorkflows to succeed, got {status} {workflows:?}"
    );
    let foreign_kind = workflows["NextToken"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("setup: two workflows must leave ListWorkflows a cursor, got {workflows:?}");
        })
        .to_string();

    // Not hex at all, odd-length hex, hex of a NUL byte, hex of "abc" — none carries the
    // marker this service's own cursors do — plus the cursor of another list.
    let bad: Vec<(&str, String)> = vec![
        ("not hex at all", "not-a-token".to_string()),
        ("odd-length hex", "zz".to_string()),
        ("hex of a NUL byte", "00".to_string()),
        ("hex of plain text", "616263".to_string()),
        ("a ListWorkflows cursor", foreign_kind),
    ];
    for (case, token) in bad {
        let (status, answered) = list(&harness, &arn, Some(2), Some(&token), None).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a 400 for a cursor this listing did not issue, got \
             {status} {answered:?}"
        );
        assert!(
            answered["__type"]
                .as_str()
                .is_some_and(|kind| kind.contains("ValidationException")),
            "case {case:?}: expected a ValidationException, got {:?}",
            answered["__type"]
        );
    }

    // The control row: the cursor this listing DOES issue is accepted, and continues the
    // walk rather than restarting it.
    let (status, first) = list(&harness, &arn, Some(2), None, None).await;
    assert_eq!(
        status, 200,
        "case own cursor: expected the first page to succeed, got {status} {first:?}"
    );
    let token = first["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("case own cursor: three runs at MaxResults 2 leave a cursor"))
        .to_string();
    let (status, second) = list(&harness, &arn, Some(2), Some(&token), None).await;
    assert_eq!(
        status, 200,
        "case own cursor: expected the second page to succeed, got {status} {second:?}"
    );
    let ids = |page: &serde_json::Value| -> Vec<String> {
        page["WorkflowRuns"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item["RunId"].as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    };
    let (head, tail) = (ids(&first), ids(&second));
    assert_eq!(
        head.len(),
        2,
        "case own cursor: expected a full first page, got {head:?}"
    );
    assert_eq!(
        tail.len(),
        1,
        "case own cursor: expected the remaining run on the second page, got {tail:?}"
    );
    assert!(
        tail.iter().all(|id| !head.contains(id)),
        "case own cursor: the second page must continue the walk, got {tail:?} after {head:?}"
    );

    // And a workflow holding no run never issues a cursor at all: the walk simply ends,
    // which is an empty page.
    let (status, empty) = list(&harness, &other, Some(2), None, None).await;
    assert_eq!(
        status, 200,
        "case other workflow: expected ListWorkflowRuns to succeed, got {status} {empty:?}"
    );
    assert_eq!(
        empty["WorkflowRuns"].as_array().map(Vec::len),
        Some(0),
        "case other workflow: a workflow with no run reports an empty page, got {:?}",
        empty["WorkflowRuns"]
    );
}
