// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the terminal state a scan run settles into.
//!
//! `GetScanResponse$scanState` "Returns either InProgress, Successful, or Failed" and
//! `GetScanResponse$errorMessage` carries "Details about the error that causes a scan to
//! fail", so the rule this suite asserts is that a run's terminal state follows from what
//! that run had to analyse, never from a fixed outcome:
//!
//! * a run started on a code artifact whose uploaded resource the service holds is
//!   carried out — it is reported `InProgress` while its transition is not yet due and
//!   `Successful` afterwards, with NO `errorMessage`, whatever scan type, analysis type or
//!   archive it was started with;
//! * a run started on an artifact nothing was uploaded to had no resource at all, so it
//!   is the one case that fails, and only that case reports an `errorMessage`;
//! * every reader of the run agrees: `GetScan` (by latest run and by `runId`), the
//!   account's scan listing (for the scans it reports — `ListScans` "Does not return
//!   `EXPRESS` scans", which `crate::list_scans_scan_type` asserts over the whole class)
//!   and the findings reader, over every revision of the scan.
//!
//! Every request goes through the real router (`MultiServiceRouter::dispatch`), the same
//! path a client takes.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_codegurusecurity_awscodegurusecurity::state::{
    EXPRESS_SCAN_SETTLE_MILLIS, SCAN_STATE_FAILED, SCAN_STATE_IN_PROGRESS, SCAN_STATE_SUCCESSFUL,
    SCAN_TYPE_EXPRESS, SCAN_TYPE_STANDARD, STANDARD_SCAN_SETTLE_MILLIS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::scan_lifecycle::{
    artifact_for, call, create_scan, harness, uploaded_artifact_for, Answer,
};
use crate::upload_target::archive_of;

/// One row of the table: a scan started the way the row describes, over the archive the
/// row uploads.
#[derive(Debug, Clone)]
struct Case {
    /// What this row varies, named in every assertion.
    label: &'static str,
    /// The `scanName` the row drives, unique per row.
    scan_name: &'static str,
    /// `scanType` as the request carries it, or `None` for the documented default.
    scan_type: Option<&'static str>,
    /// `analysisType` as the request carries it, or `None` for the documented default.
    analysis_type: Option<&'static str>,
    /// The archive uploaded to the minted target before the scan is started, or `None`
    /// for the contrast row that uploads nothing at all.
    archive: Option<Vec<u8>>,
}

impl Case {
    /// The scan type this row's run actually has, after the documented default.
    fn effective_scan_type(&self) -> &str {
        self.scan_type.unwrap_or(SCAN_TYPE_STANDARD)
    }

    /// The terminal state this row's run must reach: a run with a resource to analyse is
    /// carried out, a run without one cannot be.
    fn expected_state(&self) -> &'static str {
        if self.archive.is_some() {
            SCAN_STATE_SUCCESSFUL
        } else {
            SCAN_STATE_FAILED
        }
    }
}

async fn get_scan(router: &MultiServiceRouter, scan_name: &str, run_id: Option<&str>) -> Answer {
    let query = run_id.map(|id| format!("?runId={id}")).unwrap_or_default();
    call(router, "GET", &format!("/scans/{scan_name}{query}"), None).await
}

/// Wait past the longest clock-owed transition this suite starts, so every row's run has
/// settled. The handlers store the due instant rather than sleeping, so only the test
/// waits.
async fn wait_for_every_settle() {
    let span = STANDARD_SCAN_SETTLE_MILLIS.max(EXPRESS_SCAN_SETTLE_MILLIS);
    tokio::time::sleep(Duration::from_millis(u64::try_from(span).unwrap_or(500) + 200)).await;
}

/// The `summaries` entry `ListScans` reports for `scan_name`, or `None` when the listing
/// does not report that scan at all.
async fn listed_summary(router: &MultiServiceRouter, scan_name: &str) -> Option<Value> {
    let listed = call(router, "GET", "/scans?maxResults=100", None).await;
    assert_eq!(
        listed.status, 200,
        "ListScans: expected 200, got {} {}",
        listed.status, listed.json
    );
    listed
        .get("summaries")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|summary| summary["scanName"].as_str() == Some(scan_name))
}

/// Start the row's scan: the documented upload (where the row performs one) and then
/// `CreateScan`. Returns the `runId` of the run it started.
async fn start(router: &MultiServiceRouter, case: &Case) -> String {
    let artifact = match &case.archive {
        Some(archive) => uploaded_artifact_for(router, case.scan_name, archive.clone()).await,
        None => artifact_for(router, case.scan_name).await,
    };
    let mut body = json!({ "scanName": case.scan_name });
    if let Some(scan_type) = case.scan_type {
        body["scanType"] = json!(scan_type);
    }
    if let Some(analysis_type) = case.analysis_type {
        body["analysisType"] = json!(analysis_type);
    }
    let created = create_scan(router, body, &artifact).await;
    assert_eq!(
        created.status, 200,
        "case {label:?}: CreateScan expected 200, got {status} {json}",
        label = case.label,
        status = created.status,
        json = created.json
    );
    assert_eq!(
        created.str_of("scanState"),
        SCAN_STATE_IN_PROGRESS,
        "case {label:?}: a run that has just started is InProgress, got {json}",
        label = case.label,
        json = created.json
    );
    created.str_of("runId")
}

/// The table: rows differ along what the rule generalises over — the scan type (both
/// modelled values and the documented default), the analysis type (both modelled values
/// and the default), the SIZE and content of the uploaded archive, and whether a resource
/// was uploaded at all.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "the documented defaults over a small archive",
            scan_name: "completes-default",
            scan_type: None,
            analysis_type: None,
            archive: Some(archive_of(64, 7)),
        },
        Case {
            label: "a Standard scan with All analysis over a few KiB",
            scan_name: "completes-standard-all",
            scan_type: Some(SCAN_TYPE_STANDARD),
            analysis_type: Some("All"),
            archive: Some(archive_of(4096, 11)),
        },
        Case {
            label: "an Express scan over a small archive",
            scan_name: "completes-express",
            scan_type: Some(SCAN_TYPE_EXPRESS),
            analysis_type: None,
            archive: Some(archive_of(256, 13)),
        },
        Case {
            label: "an Express scan with All analysis over one byte",
            scan_name: "completes-express-all",
            scan_type: Some(SCAN_TYPE_EXPRESS),
            analysis_type: Some("All"),
            archive: Some(vec![b'P']),
        },
        Case {
            label: "an empty archive is still a resource that was uploaded",
            scan_name: "completes-empty-archive",
            scan_type: Some(SCAN_TYPE_STANDARD),
            analysis_type: Some("Security"),
            archive: Some(Vec::new()),
        },
        Case {
            label: "nothing was uploaded, so the run has no resource to analyse",
            scan_name: "fails-without-resource",
            scan_type: Some(SCAN_TYPE_STANDARD),
            analysis_type: None,
            archive: None,
        },
    ]
}

#[tokio::test]
async fn a_run_over_an_uploaded_resource_completes_and_only_a_failed_run_reports_an_error() {
    let (_engine, router) = harness();
    let cases = cases();

    // Every row's run is still InProgress straight after CreateScan: the transition out
    // of InProgress is owed at an instant that is never the instant of the call.
    let mut started: Vec<(Case, String)> = Vec::new();
    for case in &cases {
        let run_id = start(&router, case).await;
        let got = get_scan(&router, case.scan_name, None).await;
        assert_eq!(
            got.str_of("scanState"),
            SCAN_STATE_IN_PROGRESS,
            "case {label:?}: the run is still InProgress straight after CreateScan, got {json}",
            label = case.label,
            json = got.json
        );
        assert!(
            got.get("errorMessage").is_null(),
            "case {label:?}: a run that has not failed carries no errorMessage, got {json}",
            label = case.label,
            json = got.json
        );
        started.push((case.clone(), run_id));
    }

    wait_for_every_settle().await;

    for (case, run_id) in &started {
        let label = case.label;
        let got = get_scan(&router, case.scan_name, None).await;
        assert_eq!(
            got.status, 200,
            "case {label:?}: GetScan expected 200, got {} {}",
            got.status, got.json
        );
        assert_eq!(
            got.str_of("scanState"),
            case.expected_state(),
            "case {label:?}: expected scanState {}, got {}",
            case.expected_state(),
            got.json
        );
        if case.expected_state() == SCAN_STATE_SUCCESSFUL {
            assert!(
                got.get("errorMessage").is_null(),
                "case {label:?}: a run that did not fail carries no errorMessage, got {}",
                got.json
            );
        } else {
            assert!(
                got.get("errorMessage")
                    .as_str()
                    .is_some_and(|message| !message.is_empty()),
                "case {label:?}: a Failed run must say why it failed, got {}",
                got.json
            );
        }

        // The same run, asked for by its own id, answers with the same state.
        let by_id = get_scan(&router, case.scan_name, Some(run_id)).await;
        assert_eq!(
            (by_id.str_of("runId"), by_id.str_of("scanState")),
            (run_id.clone(), got.str_of("scanState")),
            "case {label:?}: GetScan(runId) must report the same run and state, got {}",
            by_id.json
        );

        // "`updatedAt` ... Only available for STANDARD scan types"
        // (GetScanResponse$updatedAt) — a settled run has changed, so a Standard scan
        // reports it.
        if case.effective_scan_type() == SCAN_TYPE_STANDARD {
            assert!(
                got.get("updatedAt").is_number(),
                "case {label:?}: a settled Standard scan reports updatedAt, got {}",
                got.json
            );
        } else {
            assert!(
                got.get("updatedAt").is_null(),
                "case {label:?}: updatedAt is only available for Standard scan types, got {}",
                got.json
            );
        }

        // The account's scan listing reports the state of the same run — for the scans
        // it reports at all: "Does not return EXPRESS scans" (ListScans).
        let summary = listed_summary(&router, case.scan_name).await;
        if case.effective_scan_type() == SCAN_TYPE_STANDARD {
            let summary = summary.unwrap_or_else(|| {
                panic!(
                    "case {label:?}: the listing reports no summary for the Standard scan {}",
                    case.scan_name
                )
            });
            assert_eq!(
                (summary["runId"].as_str(), summary["scanState"].as_str()),
                (Some(run_id.as_str()), Some(case.expected_state())),
                "case {label:?}: the listed summary must come from the same run, got {summary}"
            );
        } else {
            assert!(
                summary.is_none(),
                "case {label:?}: the listing does not return Express scans, got {summary:?}"
            );
        }

        // Reading a settled run must not change what the next caller sees.
        let again = get_scan(&router, case.scan_name, None).await;
        assert_eq!(
            (
                again.str_of("scanState"),
                again.get("createdAt").clone(),
                again.get("updatedAt").clone(),
                again.get("errorMessage").clone(),
            ),
            (
                got.str_of("scanState"),
                got.get("createdAt").clone(),
                got.get("updatedAt").clone(),
                got.get("errorMessage").clone(),
            ),
            "case {label:?}: two reads of one settled run must agree, got {} then {}",
            got.json,
            again.json
        );

        // The findings reader no longer conflicts once the run has finished.
        let findings = call(
            &router,
            "GET",
            &format!("/findings/{}?status=All", case.scan_name),
            None,
        )
        .await;
        assert_eq!(
            findings.status, 200,
            "case {label:?}: GetFindings on a settled run expected 200, got {} {}",
            findings.status, findings.json
        );
    }
}

#[tokio::test]
async fn every_revision_of_a_scan_settles_on_its_own_resource() {
    let (_engine, router) = harness();
    let scan_name = "revisions-settle";

    // Rows: successive revisions of ONE scan, alternating between a revision whose
    // resource was uploaded and one whose was not, so the state of a run follows that
    // run's own resource and not the scan's history.
    let revisions: [Option<Vec<u8>>; 4] = [
        Some(archive_of(128, 3)),
        None,
        Some(archive_of(32, 5)),
        Some(Vec::new()),
    ];

    let mut runs: Vec<(usize, String, &'static str)> = Vec::new();
    for (index, archive) in revisions.iter().enumerate() {
        let expected = if archive.is_some() {
            SCAN_STATE_SUCCESSFUL
        } else {
            SCAN_STATE_FAILED
        };
        let artifact = match archive {
            Some(bytes) => uploaded_artifact_for(&router, scan_name, bytes.clone()).await,
            None => artifact_for(&router, scan_name).await,
        };
        let created = create_scan(&router, json!({ "scanName": scan_name }), &artifact).await;
        assert_eq!(
            created.status, 200,
            "revision {index}: CreateScan expected 200, got {} {}",
            created.status, created.json
        );
        let run_id = created.str_of("runId");
        runs.push((index, run_id, expected));

        // A revision cannot start while the previous one is running, so each row waits
        // for its own run to settle before the next starts.
        wait_for_every_settle().await;

        let got = get_scan(&router, scan_name, None).await;
        assert_eq!(
            got.get("numberOfRevisions").as_i64(),
            Some(i64::try_from(index).unwrap_or_default()),
            "revision {index}: numberOfRevisions counts the re-runs, got {}",
            got.json
        );
    }

    // Every earlier run keeps the state its own resource gave it.
    for (index, run_id, expected) in &runs {
        let got = get_scan(&router, scan_name, Some(run_id)).await;
        assert_eq!(
            got.str_of("scanState"),
            *expected,
            "revision {index}: expected scanState {expected}, got {}",
            got.json
        );
        assert_eq!(
            got.get("errorMessage").is_null(),
            *expected == SCAN_STATE_SUCCESSFUL,
            "revision {index}: errorMessage belongs to a failed run only, got {}",
            got.json
        );
    }
}
