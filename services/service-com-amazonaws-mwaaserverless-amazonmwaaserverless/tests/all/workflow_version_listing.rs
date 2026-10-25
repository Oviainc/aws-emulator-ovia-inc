// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListWorkflowVersions`.
//!
//! The rule under test: `ListWorkflowVersions` reports one `WorkflowVersionSummary` per
//! immutable version the STORE holds under the workflow its `WorkflowArn` names — in the
//! order those versions were appended, at most `MaxResults` per page, continued through
//! the opaque `NextToken` this service itself issued. Each summary carries the stored
//! version's own identifier and instants plus the configuration THAT version froze — the
//! same configuration `GetWorkflow` serves when it is asked for that version by name —
//! and exactly one of them is flagged `IsLatestVersion`, since an update "creates a new
//! version with the updated configuration and disables scheduling on all previous
//! versions" (`smithy_query(query="operation", operation="UpdateWorkflow")`). A workflow
//! this endpoint does not hold contributes no version, so the listing answers an empty
//! page rather than an error shape its modelled error set does not declare (the suite
//! `child_listing_error_set` carries that rule across both child-resource listings), and
//! the listing persists nothing.
//!
//! The tables span what the rule generalises over: how many versions a workflow holds,
//! the page size (including both published `MaxResults` bounds and the modelled
//! default), the configuration each version froze, which ARNs do not resolve, and the
//! form of a `NextToken`.

use std::time::Duration;

use service_com_amazonaws_mwaaserverless_amazonmwaaserverless::state::WORKFLOW_DELETED_AFTER;

use crate::missing_resource_lookups::ghost_arns;
use crate::workflow_lifecycle::{role_arn, Harness};

/// The members a version summary and a `GetWorkflow` read of that SAME version must
/// agree on, because both answer from the one stored snapshot. From
/// `smithy_query(query="shape", shape="WorkflowVersionSummary")` intersected with
/// `smithy_query(query="operation", operation="GetWorkflow")`.
const SHARED_MEMBERS: &[&str] = &[
    "WorkflowVersion",
    "WorkflowArn",
    "DefinitionS3Location",
    "TriggerMode",
];

/// Create a workflow and report its ARN.
async fn created_workflow(harness: &Harness, name: &str) -> String {
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
        "setup: expected CreateWorkflow to succeed, got {status} {created:?}"
    );
    created["WorkflowArn"]
        .as_str()
        .expect("setup: a created workflow has an ARN")
        .to_string()
}

/// The configurations the version history is built from. Rows differ along what a
/// version summary reports: the S3 bucket and object key, whether the object's own
/// `VersionId` is supplied, and the `TriggerMode` (`GenericString`, length 1..255,
/// pattern `^.*$`, so any non-empty text is a legal value).
fn version_configs() -> Vec<(&'static str, serde_json::Value)> {
    vec![
        (
            "bucket and key only",
            serde_json::json!({
                "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": "dags/v1.yaml"},
            }),
        ),
        (
            "an S3 object version and a trigger mode",
            serde_json::json!({
                "DefinitionS3Location": {
                    "Bucket": "defs",
                    "ObjectKey": "dags/v2.yaml",
                    "VersionId": "s3objectversion2",
                },
                "TriggerMode": "SCHEDULED",
            }),
        ),
        (
            "a different bucket and a nested key",
            serde_json::json!({
                "DefinitionS3Location": {
                    "Bucket": "other-definitions",
                    "ObjectKey": "team/data/dags/v3.yaml",
                },
                "TriggerMode": "EVENT",
            }),
        ),
        (
            "a one-character trigger mode, the shortest the constraint allows",
            serde_json::json!({
                "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": "dags/v4.yaml"},
                "TriggerMode": "M",
            }),
        ),
        (
            "the same location as an earlier version, with no trigger mode",
            serde_json::json!({
                "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": "dags/v1.yaml"},
            }),
        ),
        (
            "a description, which a summary does not report",
            serde_json::json!({
                "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": "dags/v6.yaml"},
                "Description": "the sixth configuration",
                "TriggerMode": "MANUAL",
            }),
        ),
    ]
}

/// Append one version to a workflow from a configuration row.
async fn appended_version(harness: &Harness, arn: &str, case: &str, config: &serde_json::Value) {
    let mut body = config.clone();
    body["WorkflowArn"] = serde_json::Value::String(arn.to_string());
    body["RoleArn"] = serde_json::Value::String(role_arn());
    let (status, updated) = harness.call("UpdateWorkflow", body).await;
    assert_eq!(
        status, 200,
        "setup {case:?}: expected UpdateWorkflow to succeed, got {status} {updated:?}"
    );
}

/// Call `ListWorkflowVersions` over the wire.
async fn list(
    harness: &Harness,
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
    harness.call("ListWorkflowVersions", body).await
}

/// Page through the whole listing at `page_size` and report every `WorkflowVersion` in
/// the order the pages reported them.
async fn paged_versions(
    harness: &Harness,
    arn: &str,
    page_size: i64,
    case: &str,
) -> Vec<serde_json::Value> {
    let mut seen: Vec<serde_json::Value> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0;
    loop {
        let (status, page) = list(harness, arn, Some(page_size), token.as_deref()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListWorkflowVersions to succeed, got {status} {page:?}"
        );
        let items = page["WorkflowVersions"]
            .as_array()
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected a WorkflowVersions list, got {page:?}");
            })
            .clone();
        assert!(
            i64::try_from(items.len()).is_ok_and(|count| count <= page_size),
            "case {case:?}: a page carried {} items for MaxResults {page_size}",
            items.len()
        );
        seen.extend(items.iter().cloned());
        pages += 1;
        assert!(
            pages <= 32,
            "case {case:?}: pagination did not terminate, saw {} items",
            seen.len()
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

/// The `WorkflowVersion` of each summary in a page, in the order reported.
fn version_ids(items: &[serde_json::Value]) -> Vec<String> {
    items
        .iter()
        .map(|item| {
            item["WorkflowVersion"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("a summary must carry its WorkflowVersion, got {item:?}");
                })
                .to_string()
        })
        .collect()
}

#[tokio::test]
async fn every_stored_version_is_listed_exactly_once_across_pages_in_append_order() {
    let harness = Harness::new("us-east-1");

    // The rows differ along HOW MANY versions the workflow holds: the one a create
    // mints on its own, and that one plus each further configuration appended.
    for appended in [0_usize, 1, 2, 6] {
        let arn = created_workflow(&harness, &format!("pagedVersions{appended}")).await;
        for (case, config) in version_configs().into_iter().take(appended) {
            appended_version(&harness, &arn, case, &config).await;
        }
        let expected: Vec<String> = harness
            .stored_versions(&arn)
            .into_iter()
            .map(|(version, _)| version)
            .collect();
        assert_eq!(
            expected.len(),
            appended + 1,
            "setup {appended}: a create mints one version and each update appends one, \
             got {expected:?}"
        );

        // `MaxResults` is modelled `@range(min: 1, max: 100)`: both published bounds plus
        // sizes smaller than, equal to and larger than the set.
        for page_size in [1_i64, 2, 3, 7, 100] {
            let case = format!("{appended} appended/MaxResults={page_size}");
            let seen = paged_versions(&harness, &arn, page_size, &case).await;
            assert_eq!(
                version_ids(&seen),
                expected,
                "case {case:?}: paging must yield every version exactly once, in the order \
                 they were appended, got {:?}",
                version_ids(&seen)
            );
        }

        // The modelled default `MaxResults` of 20 covers every set above in one page, and
        // a complete page carries no cursor.
        let (status, defaulted) = list(&harness, &arn, None, None).await;
        assert_eq!(
            status, 200,
            "case {appended} appended/default MaxResults: expected success, got {status} \
             {defaulted:?}"
        );
        assert_eq!(
            defaulted["WorkflowVersions"].as_array().map(Vec::len),
            Some(expected.len()),
            "case {appended} appended/default MaxResults: the modelled default of 20 must \
             cover all {}, got {:?}",
            expected.len(),
            defaulted["WorkflowVersions"]
        );
        assert!(
            defaulted["NextToken"].is_null(),
            "case {appended} appended/default MaxResults: a complete page carries no \
             NextToken, got {:?}",
            defaulted["NextToken"]
        );

        // The listing is a read: it neither appends nor drops a version, so the store and
        // a second listing answer exactly as before.
        let after: Vec<String> = harness
            .stored_versions(&arn)
            .into_iter()
            .map(|(version, _)| version)
            .collect();
        assert_eq!(
            after, expected,
            "case {appended} appended: a listing must not change the stored version \
             history, got {after:?}"
        );
        let (status, again) = list(&harness, &arn, Some(100), None).await;
        assert_eq!(
            status, 200,
            "case {appended} appended/repeated: expected success, got {status} {again:?}"
        );
        assert_eq!(
            again["WorkflowVersions"], defaulted["WorkflowVersions"],
            "case {appended} appended/repeated: a read must not change what a later caller \
             sees, got {:?}",
            again["WorkflowVersions"]
        );
    }
}

#[tokio::test]
async fn exactly_one_listed_version_is_flagged_latest_and_it_is_the_newest() {
    let harness = Harness::new("eu-west-1");
    let arn = created_workflow(&harness, "latestFlag").await;

    // After the create and after EVERY appended version, the flag must follow the newest
    // version — so the rows are the history as it grows, one configuration at a time.
    let mut appended = 0_usize;
    loop {
        let case = format!("{appended} appended");
        let stored = harness.stored_versions(&arn);
        let (status, listed) = list(&harness, &arn, Some(100), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListWorkflowVersions to succeed, got {status} {listed:?}"
        );
        let items = listed["WorkflowVersions"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected a list, got {listed:?}"))
            .clone();

        let flagged: Vec<String> = items
            .iter()
            .filter(|item| item["IsLatestVersion"].as_bool() == Some(true))
            .map(|item| item["WorkflowVersion"].as_str().unwrap_or("").to_string())
            .collect();
        assert_eq!(
            flagged.len(),
            1,
            "case {case:?}: exactly one version is the latest, got {flagged:?}"
        );
        let newest = stored
            .last()
            .map(|(version, _)| version.clone())
            .unwrap_or_default();
        assert_eq!(
            flagged.first().map(String::as_str),
            Some(newest.as_str()),
            "case {case:?}: the newest appended version is the latest one, got {flagged:?}"
        );
        // Every other version reports the flag as false rather than omitting it, so a
        // client never has to guess.
        for item in &items {
            let version = item["WorkflowVersion"].as_str().unwrap_or("");
            let expected = version == newest;
            assert_eq!(
                item["IsLatestVersion"].as_bool(),
                Some(expected),
                "case {case:?}: expected IsLatestVersion {expected} for {version:?}, got {:?}",
                item["IsLatestVersion"]
            );
        }
        // The listing agrees with the store on both the identifier and the flag.
        let listed_pairs: Vec<(String, bool)> = items
            .iter()
            .map(|item| {
                (
                    item["WorkflowVersion"].as_str().unwrap_or("").to_string(),
                    item["IsLatestVersion"].as_bool().unwrap_or(false),
                )
            })
            .collect();
        assert_eq!(
            listed_pairs, stored,
            "case {case:?}: the listing must report what the store holds, got {listed_pairs:?}"
        );

        let configs = version_configs();
        if appended >= configs.len() {
            break;
        }
        let (label, config) = &configs[appended];
        appended_version(&harness, &arn, label, config).await;
        appended += 1;
    }
}

#[tokio::test]
async fn a_listed_summary_reports_the_configuration_that_version_froze() {
    let harness = Harness::new("ap-south-1");
    let arn = created_workflow(&harness, "summaryFidelity").await;
    for (case, config) in version_configs() {
        appended_version(&harness, &arn, case, &config).await;
    }

    let (status, listed) = list(&harness, &arn, Some(100), None).await;
    assert_eq!(
        status, 200,
        "expected ListWorkflowVersions to succeed, got {status} {listed:?}"
    );
    let items = listed["WorkflowVersions"]
        .as_array()
        .expect("the listing reports a WorkflowVersions list")
        .clone();
    assert_eq!(
        items.len(),
        version_configs().len() + 1,
        "expected one summary per stored version, got {items:?}"
    );

    for item in &items {
        let version = item["WorkflowVersion"]
            .as_str()
            .unwrap_or_else(|| panic!("a summary carries its WorkflowVersion, got {item:?}"));
        let case = format!("version {version}");

        // The modelled `WorkflowVersion`: length 32..32, pattern `^[a-z0-9]{32}$`.
        assert!(
            version.len() == 32
                && version
                    .bytes()
                    .all(|b| b.is_ascii_digit() || b.is_ascii_lowercase()),
            "case {case:?}: expected the modelled WorkflowVersion form, got {version:?}"
        );
        assert_eq!(
            item["WorkflowArn"].as_str(),
            Some(arn.as_str()),
            "case {case:?}: a summary reports the workflow that holds the version, got {:?}",
            item["WorkflowArn"]
        );

        // A version is immutable, so the instant it was written is also the last instant
        // it changed.
        assert_eq!(
            item["CreatedAt"], item["ModifiedAt"],
            "case {case:?}: an immutable version reports one instant for both, got {:?} \
             and {:?}",
            item["CreatedAt"], item["ModifiedAt"]
        );

        // `GetWorkflow` asked for THIS version by name answers from the same frozen
        // snapshot, so the two reads cannot disagree on any shared member.
        let (status, got) = harness
            .call(
                "GetWorkflow",
                serde_json::json!({"WorkflowArn": arn, "WorkflowVersion": version}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetWorkflow to succeed, got {status} {got:?}"
        );
        for member in SHARED_MEMBERS {
            assert_eq!(
                item[member], got[member],
                "case {case:?}: expected the summary's {member} to match GetWorkflow's {:?}, \
                 got {:?}",
                got[member], item[member]
            );
        }
        // `GetWorkflow`'s `ModifiedAt` for a NAMED version is that version's own instant,
        // which is what the summary reports as `CreatedAt`.
        assert_eq!(
            item["CreatedAt"], got["ModifiedAt"],
            "case {case:?}: expected the version's own instant, got {:?} against {:?}",
            item["CreatedAt"], got["ModifiedAt"]
        );
        // No modelled operation of this service accepts a schedule, so no stored version
        // holds one and neither read may invent one.
        assert!(
            item["ScheduleConfiguration"].is_null() && got["ScheduleConfiguration"].is_null(),
            "case {case:?}: no stored version holds a schedule, got {:?}",
            item["ScheduleConfiguration"]
        );
    }

    // Each appended configuration is reported by SOME version, in the order appended: the
    // definition locations the rows differ along must come back distinct and in sequence.
    let locations: Vec<serde_json::Value> = items
        .iter()
        .map(|item| item["DefinitionS3Location"].clone())
        .collect();
    let mut expected: Vec<serde_json::Value> =
        vec![serde_json::json!({"Bucket": "defs", "ObjectKey": "dags/v0.yaml"})];
    for (_, config) in version_configs() {
        expected.push(config["DefinitionS3Location"].clone());
    }
    assert_eq!(
        locations, expected,
        "the listing must report each version's own frozen definition location, got \
         {locations:?}"
    );
}

#[tokio::test]
async fn a_workflow_the_store_does_not_hold_is_an_empty_page() {
    let harness = Harness::new("us-west-2");

    // A workflow that existed and was torn down is as absent as one that never did.
    let deleted = created_workflow(&harness, "listedThenDeleted").await;
    let (status, removed) = harness
        .call("DeleteWorkflow", serde_json::json!({"WorkflowArn": deleted}))
        .await;
    assert_eq!(
        status, 200,
        "setup: expected DeleteWorkflow to succeed, got {status} {removed:?}"
    );
    tokio::time::sleep(WORKFLOW_DELETED_AFTER + Duration::from_millis(200)).await;

    let mut arns = ghost_arns();
    arns.push(deleted.clone());

    for arn in &arns {
        // The answer does not depend on the other members: with and without a page size.
        for (case, size) in [("plain", None), ("with a page size", Some(1_i64))] {
            let label = format!("{case}/{arn}");
            let (status, answered) = list(&harness, arn, size, None).await;
            assert_eq!(
                status, 200,
                "case {label:?}: expected HTTP 200 for a workflow this endpoint does not \
                 hold, got {status} {answered:?}"
            );
            assert_eq!(
                answered["WorkflowVersions"].as_array().map(Vec::len),
                Some(0),
                "case {label:?}: expected an empty page, got {:?}",
                answered["WorkflowVersions"]
            );
            assert!(
                answered["NextToken"].is_null(),
                "case {label:?}: an empty page ends the walk, got {:?}",
                answered["NextToken"]
            );
        }
    }

    // The listing creates nothing: the torn-down workflow stays gone.
    assert!(
        harness.stored_versions(&deleted).is_empty(),
        "listing an absent workflow must not resurrect its versions, got {:?}",
        harness.stored_versions(&deleted)
    );
}

#[tokio::test]
async fn a_pagination_cursor_this_listing_did_not_issue_is_refused() {
    let harness = Harness::new("eu-central-1");
    let arn = created_workflow(&harness, "cursorProbe").await;
    let other = created_workflow(&harness, "cursorProbeOther").await;
    for (case, config) in version_configs().into_iter().take(2) {
        appended_version(&harness, &arn, case, &config).await;
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

    // And a cursor this very listing issued for a DIFFERENT workflow: it names a version
    // the listed workflow does not hold, so continuing from it would silently truncate or
    // restart the walk.
    let (status, first_of_arn) = list(&harness, &arn, Some(1), None).await;
    assert_eq!(
        status, 200,
        "setup: expected the first page to succeed, got {status} {first_of_arn:?}"
    );
    let cursor_of_another_workflow = first_of_arn["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("setup: three versions at MaxResults 1 leave a cursor"))
        .to_string();

    // Not hex at all, odd-length hex, hex of a NUL byte, hex of plain text — none carries
    // the marker this service's own cursors do — plus the cursor of another list and the
    // cursor of another workflow's version listing.
    let bad: Vec<(&str, String)> = vec![
        ("not hex at all", "not-a-token".to_string()),
        ("odd-length hex", "616".to_string()),
        ("hex of a NUL byte", "00".to_string()),
        ("hex of plain text", "616263".to_string()),
        ("a ListWorkflows cursor", foreign_kind),
        (
            "a cursor from another workflow's version listing",
            cursor_of_another_workflow,
        ),
    ];
    for (case, token) in bad {
        let (status, answered) = list(&harness, &other, Some(2), Some(&token)).await;
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
        assert!(
            answered["WorkflowVersions"].is_null(),
            "case {case:?}: a refusal reports no page, got {:?}",
            answered["WorkflowVersions"]
        );
    }

    // The control row: the cursor this listing DOES issue is accepted, and continues the
    // walk rather than restarting it.
    let (status, first) = list(&harness, &arn, Some(2), None).await;
    assert_eq!(
        status, 200,
        "case own cursor: expected the first page to succeed, got {status} {first:?}"
    );
    let token = first["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("case own cursor: three versions at MaxResults 2 leave a cursor"))
        .to_string();
    let (status, second) = list(&harness, &arn, Some(2), Some(&token)).await;
    assert_eq!(
        status, 200,
        "case own cursor: expected the second page to succeed, got {status} {second:?}"
    );
    let head = version_ids(first["WorkflowVersions"].as_array().unwrap_or(&Vec::new()));
    let tail = version_ids(second["WorkflowVersions"].as_array().unwrap_or(&Vec::new()));
    assert_eq!(
        head.len(),
        2,
        "case own cursor: expected a full first page, got {head:?}"
    );
    assert_eq!(
        tail.len(),
        1,
        "case own cursor: expected the remaining version on the second page, got {tail:?}"
    );
    assert!(
        tail.iter().all(|id| !head.contains(id)),
        "case own cursor: the second page must continue the walk, got {tail:?} after {head:?}"
    );

    // A workflow holding only the version its create minted never issues a cursor at all:
    // one page covers it and the walk ends.
    let (status, single) = list(&harness, &other, Some(2), None).await;
    assert_eq!(
        status, 200,
        "case other workflow: expected ListWorkflowVersions to succeed, got {status} {single:?}"
    );
    assert_eq!(
        single["WorkflowVersions"].as_array().map(Vec::len),
        Some(1),
        "case other workflow: a create mints exactly one version, got {:?}",
        single["WorkflowVersions"]
    );
    assert!(
        single["NextToken"].is_null(),
        "case other workflow: a complete page carries no NextToken, got {:?}",
        single["NextToken"]
    );
}
