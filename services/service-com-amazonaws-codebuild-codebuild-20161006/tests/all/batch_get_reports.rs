// SPDX-License-Identifier: CC0-1.0

//! What `BatchGetReports` answers for the report ARNs a caller asks about, driven over
//! the wire the router serves: `POST /` with
//! `X-Amz-Target: CodeBuild_20161006.BatchGetReports` on awsJson1_1.
//!
//! Two rules, each asserted over a table rather than over one request:
//!
//! * a value that is not a report ARN of this service identifies no report at all, so
//!   the whole call is refused with 400 `InvalidInputException` and CodeBuild's own
//!   fixed sentence;
//! * a value that IS a report ARN is answered from the store — the record it keys,
//!   reported verbatim, or the ARN in `reportsNotFound` when the account holds none —
//!   positionally, and a read leaves what a later caller sees unchanged.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_codebuild_codebuild_20161006::ops::types::{
    CodeCoverageReportSummary, Report, TestReportSummary, REPORT_STATUS_TYPE_VALUES,
    REPORT_TYPE_VALUES,
};
use service_com_amazonaws_codebuild_codebuild_20161006::state::{
    self, ComAmazonawsCodebuildCodebuild20161006State as CodeBuildState,
};
use smithy_server_runtime::MultiServiceRouter;

// The harness every suite in this test crate drives the service through: one router
// over an engine this test holds, and one awsJson1_1 call signed for [`REGION`].
use super::project_build_lifecycle::{call, emulator, emulator_on, error_type, REGION};

/// The instant the seeded records report as their creation time, as epoch millis: a
/// fixed value, so the assertion is on what the store holds rather than on a clock.
const SEEDED_CREATED_MILLIS: i64 = 1_700_000_000_000;

/// When a seeded record reports it expires. AWS documents the span on `Report$expired`:
/// "A report expires 30 days after it is created."
const SEEDED_EXPIRED_MILLIS: i64 = SEEDED_CREATED_MILLIS + 30 * 24 * 60 * 60 * 1_000;

/// Read the reports `arns` address, asserting the call succeeded.
async fn batch_get(router: &MultiServiceRouter, arns: &[String]) -> Value {
    let (status, body) = call(router, "BatchGetReports", json!({ "reportArns": arns })).await;
    assert_eq!(
        status, 200,
        "BatchGetReports {arns:?}: expected 200, got {status} with {body}"
    );
    body
}

/// Ask about `arns`, assert the call was refused with 400 `InvalidInputException`, and
/// hand back the sentence it stated.
async fn refused(router: &MultiServiceRouter, case: &str, arns: &[String]) -> String {
    let (status, body) = call(router, "BatchGetReports", json!({ "reportArns": arns })).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected 400 from BatchGetReports, got {status} with {body}"
    );
    assert_eq!(
        error_type(&body),
        "InvalidInputException",
        "case {case:?}: expected InvalidInputException, got {body}"
    );
    body["message"].as_str().unwrap_or_default().to_string()
}

/// A report of `report_group` whose id is `report_id`, as the store holds one: its ARN
/// comes from the service's own minter, so the test cannot drift from the ARN form the
/// handlers resolve.
fn seeded_report(report_group: &str, report_id: &str, report_type: &str, status: &str) -> Report {
    let test_report = report_type == "TEST";
    Report {
        arn: Some(state::report_arn(REGION, report_group, report_id)),
        r#type: Some(report_type.to_string()),
        name: Some(report_group.to_string()),
        report_group_arn: Some(state::report_group_arn(REGION, report_group)),
        execution_id: Some(state::build_arn(REGION, &format!("{report_group}:{report_id}"))),
        status: Some(status.to_string()),
        created: Some(state::datetime(SEEDED_CREATED_MILLIS)),
        expired: Some(state::datetime(SEEDED_EXPIRED_MILLIS)),
        export_config: None,
        truncated: Some(false),
        test_summary: test_report.then(|| TestReportSummary {
            total: 3,
            status_counts: HashMap::from([("SUCCEEDED".to_string(), 2), ("FAILED".to_string(), 1)]),
            duration_in_nano_seconds: 4_200_000_000,
        }),
        code_coverage_summary: (!test_report).then_some(CodeCoverageReportSummary {
            line_coverage_percentage: Some(87.5),
            lines_covered: Some(35),
            lines_missed: Some(5),
            branch_coverage_percentage: Some(50.0),
            branches_covered: Some(2),
            branches_missed: Some(2),
        }),
    }
}

/// An engine whose CodeBuild state holds `reports`, written straight into the store
/// rather than through an operation — so what a read answers can only have come from the
/// store. Reports are keyed by their own ARN, which is how every report operation
/// addresses one.
fn engine_holding(reports: Vec<Report>) -> Arc<engine::InMemoryEngine> {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let handle = engine.state::<CodeBuildState>();
    let mut guard = handle.lock();
    for report in reports {
        let arn = report.arn.clone().expect("a seeded report carries its ARN");
        guard.reports.insert(arn, report);
    }
    drop(guard);
    engine
}

/// RULE: a report ARN is answered from the store — the record it is the key of, reported
/// verbatim, with nothing recomputed from the request — and a repeated read answers the
/// same thing, so reading changes nothing a later caller sees.
///
/// The two rows are the model's whole `ReportType` catalogue (`REPORT_TYPE_VALUES`), one
/// of them under a report group name carrying a colon — which `ReportGroupName` permits,
/// constraining only its length — so the group name and the report id are recovered from
/// the ARN rather than guessed at its first separator.
#[tokio::test]
async fn a_read_reports_the_stored_report_verbatim() {
    let test = seeded_report("unit-tests", "a1b2c3d4", "TEST", "SUCCEEDED");
    let coverage = seeded_report("suite:coverage", "e5f6a7b8", "CODE_COVERAGE", "INCOMPLETE");
    let engine = engine_holding(vec![test.clone(), coverage.clone()]);
    let router = emulator_on(Arc::clone(&engine));

    let asked = [
        coverage.arn.clone().expect("an ARN"),
        test.arn.clone().expect("an ARN"),
    ];
    let read = batch_get(&router, &asked).await;
    assert_eq!(
        read["reports"],
        json!([
            {
                // awsJson1_1 renders a timestamp as epoch SECONDS; the absent
                // `exportConfig` of the stored record stays absent.
                "arn": asked[0],
                "type": "CODE_COVERAGE",
                "name": "suite:coverage",
                "reportGroupArn": state::report_group_arn(REGION, "suite:coverage"),
                "executionId": state::build_arn(REGION, "suite:coverage:e5f6a7b8"),
                "status": "INCOMPLETE",
                "created": SEEDED_CREATED_MILLIS / 1_000,
                "expired": SEEDED_EXPIRED_MILLIS / 1_000,
                "truncated": false,
                "codeCoverageSummary": {
                    "lineCoveragePercentage": 87.5,
                    "linesCovered": 35,
                    "linesMissed": 5,
                    "branchCoveragePercentage": 50.0,
                    "branchesCovered": 2,
                    "branchesMissed": 2,
                },
            },
            {
                "arn": asked[1],
                "type": "TEST",
                "name": "unit-tests",
                "reportGroupArn": state::report_group_arn(REGION, "unit-tests"),
                "executionId": state::build_arn(REGION, "unit-tests:a1b2c3d4"),
                "status": "SUCCEEDED",
                "created": SEEDED_CREATED_MILLIS / 1_000,
                "expired": SEEDED_EXPIRED_MILLIS / 1_000,
                "truncated": false,
                "testSummary": {
                    "total": 3,
                    "statusCounts": {"SUCCEEDED": 2, "FAILED": 1},
                    "durationInNanoSeconds": 4_200_000_000i64,
                },
            },
        ]),
        "the read must report the stored records verbatim, in request order, got {read}"
    );
    assert_eq!(
        read["reportsNotFound"],
        json!([]),
        "a held ARN must not be reported missing, got {read}"
    );

    let again = batch_get(&router, &asked).await;
    assert_eq!(
        again, read,
        "a second read must answer the same records, got {again}"
    );
    let held = engine.state::<CodeBuildState>().lock().reports.len();
    assert_eq!(
        held, 2,
        "a read must not add to or drop from the store, got {held} record(s)"
    );
}

/// RULE: every pair of the modelled `ReportType` and `ReportStatusType` catalogues
/// (`REPORT_TYPE_VALUES`, `REPORT_STATUS_TYPE_VALUES` — the model's own value lists) is
/// reported as stored: the reader carries a report's type and status through unchanged
/// rather than deciding them, so no value of either catalogue is special-cased.
#[tokio::test]
async fn every_modelled_type_and_status_is_reported_as_stored() {
    let mut seeded: Vec<Report> = Vec::new();
    for report_type in REPORT_TYPE_VALUES {
        for status in REPORT_STATUS_TYPE_VALUES {
            let group = format!("catalogue-{}", report_type.to_lowercase());
            seeded.push(seeded_report(
                &group,
                &status.to_lowercase(),
                report_type,
                status,
            ));
        }
    }
    let router = emulator_on(engine_holding(seeded.clone()));

    let asked: Vec<String> = seeded
        .iter()
        .map(|report| report.arn.clone().expect("an ARN"))
        .collect();
    let read = batch_get(&router, &asked).await;
    let reported = read["reports"].as_array().expect("reports is a list");
    assert_eq!(
        reported.len(),
        seeded.len(),
        "every seeded report must be reported, got {read}"
    );
    for (report, answered) in seeded.iter().zip(reported) {
        let case = report.arn.clone().unwrap_or_default();
        assert_eq!(
            answered["type"],
            json!(report.r#type),
            "case {case:?}: expected the stored type, got {answered}"
        );
        assert_eq!(
            answered["status"],
            json!(report.status),
            "case {case:?}: expected the stored status, got {answered}"
        );
        assert_eq!(
            answered["arn"],
            json!(case),
            "case {case:?}: expected the ARN asked about, got {answered}"
        );
    }
}

/// RULE: a report ARN names a report only when the store holds a record under exactly
/// that ARN, so a well-formed report ARN differing in any field — partition, region,
/// account, report group, report id, or the case of either — resolves to nothing and is
/// reported in `reportsNotFound`, never refused and never resolved to a near neighbour.
#[tokio::test]
async fn only_the_exact_stored_report_arn_resolves() {
    let held = seeded_report("held-group", "0abc1234", "TEST", "SUCCEEDED");
    let held_arn = held.arn.clone().expect("an ARN");
    let router = emulator_on(engine_holding(vec![held]));
    let account = engine::EMULATOR_ACCOUNT_ID;

    // (case, the ARN asked about, whether the store holds it)
    let rows: Vec<(&str, String, bool)> = vec![
        ("the stored ARN itself", held_arn.clone(), true),
        (
            "another partition",
            format!("arn:aws-cn:codebuild:{REGION}:{account}:report/held-group:0abc1234"),
            false,
        ),
        (
            "another region",
            format!("arn:aws:codebuild:eu-west-1:{account}:report/held-group:0abc1234"),
            false,
        ),
        (
            "another account",
            format!("arn:aws:codebuild:{REGION}:111122223333:report/held-group:0abc1234"),
            false,
        ),
        (
            "another report group",
            state::report_arn(REGION, "other-group", "0abc1234"),
            false,
        ),
        (
            "another report id of the held group",
            state::report_arn(REGION, "held-group", "0abc9999"),
            false,
        ),
        (
            "the report group in another case",
            state::report_arn(REGION, "Held-Group", "0abc1234"),
            false,
        ),
        (
            "the report id in another case",
            state::report_arn(REGION, "held-group", "0ABC1234"),
            false,
        ),
    ];

    for (case, arn, stored) in &rows {
        let read = batch_get(&router, std::slice::from_ref(arn)).await;
        if *stored {
            assert_eq!(
                read["reports"][0]["arn"],
                json!(arn),
                "case {case:?}: expected the stored record, got {read}"
            );
            assert_eq!(
                read["reportsNotFound"],
                json!([]),
                "case {case:?}: a held ARN must not be reported missing, got {read}"
            );
        } else {
            assert_eq!(
                read["reportsNotFound"],
                json!([arn]),
                "case {case:?}: expected the ARN reported missing, got {read}"
            );
            // Both halves of the partition are always serialized: the found half is
            // `[]`, not absent.
            assert_eq!(
                read["reports"],
                json!([]),
                "case {case:?}: expected an empty reports, got {read}"
            );
        }
    }

    // One batch over the whole table: each ARN lands in exactly one list, in request
    // order, and a REPEATED ARN is answered once per position rather than collapsed — so
    // a client can zip the answer against what it asked for.
    let mut asked: Vec<String> = rows.iter().map(|(_, arn, _)| arn.clone()).collect();
    asked.push(held_arn.clone());
    let read = batch_get(&router, &asked).await;
    let reported: Vec<String> = read["reports"]
        .as_array()
        .expect("reports is a list")
        .iter()
        .map(|report| report["arn"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        reported,
        vec![held_arn.clone(), held_arn.clone()],
        "a repeated held ARN must be answered once per position, got {read}"
    );
    let missing: Vec<String> = read["reportsNotFound"]
        .as_array()
        .expect("reportsNotFound is a list")
        .iter()
        .map(|arn| arn.as_str().unwrap_or_default().to_string())
        .collect();
    let expected_missing: Vec<String> = rows
        .iter()
        .filter(|(_, _, stored)| !stored)
        .map(|(_, arn, _)| arn.clone())
        .collect();
    assert_eq!(
        missing, expected_missing,
        "every ARN the store does not hold must be answered in request order, got {read}"
    );
    assert_eq!(
        reported.len() + missing.len(),
        asked.len(),
        "the two lists must cover every ARN of the request, got {read}"
    );
}

/// RULE: a value that is not a report ARN of this service identifies no report in any
/// account or region, so the whole call is refused with 400 `InvalidInputException`
/// worded `Invalid report ARN` — wholly fixed text that quotes neither the rejected
/// value, the member that carried it nor the ARN form that would be accepted — rather
/// than reported as a report that merely is not held.
///
/// The table spans the forms a value can be malformed in: not an ARN at all, an ARN of
/// another service or of another resource of this one, a report ARN missing either half
/// of its resource, and a truncated ARN.
#[tokio::test]
async fn a_value_that_is_not_a_report_arn_is_refused_in_codebuilds_own_words() {
    let held = seeded_report("held-group", "0abc1234", "TEST", "SUCCEEDED");
    let held_arn = held.arn.clone().expect("an ARN");
    let engine = engine_holding(vec![held]);
    let router = emulator_on(Arc::clone(&engine));
    let account = engine::EMULATOR_ACCOUNT_ID;

    // (case, the value asked about)
    let malformed: Vec<(&str, String)> = vec![
        ("a bare report group name", "held-group".to_string()),
        (
            "a report group name and id, unqualified",
            "held-group:0abc1234".to_string(),
        ),
        (
            "the report group's own ARN",
            state::report_group_arn(REGION, "held-group"),
        ),
        (
            "a project ARN of this service",
            format!("arn:aws:codebuild:{REGION}:{account}:project/held-group"),
        ),
        (
            "a build ARN of this service",
            state::build_arn(REGION, "held-group:0abc1234"),
        ),
        (
            "another service's ARN",
            format!("arn:aws:codepipeline:{REGION}:{account}:report/held-group:0abc1234"),
        ),
        (
            "a report resource with no id",
            format!("arn:aws:codebuild:{REGION}:{account}:report/held-group"),
        ),
        (
            "a report resource with an empty id",
            format!("arn:aws:codebuild:{REGION}:{account}:report/held-group:"),
        ),
        (
            "a report resource with an empty group",
            format!("arn:aws:codebuild:{REGION}:{account}:report/:0abc1234"),
        ),
        (
            "a report resource with nothing after the type",
            format!("arn:aws:codebuild:{REGION}:{account}:report/"),
        ),
        (
            "an empty partition",
            format!("arn::codebuild:{REGION}:{account}:report/held-group:0abc1234"),
        ),
        (
            "a truncated ARN",
            format!("arn:aws:codebuild:{REGION}:{account}"),
        ),
        (
            "the resource segment alone",
            "report/held-group:0abc1234".to_string(),
        ),
    ];

    for (case, value) in &malformed {
        let stated = refused(&router, case, std::slice::from_ref(value)).await;
        assert_eq!(
            stated, "Invalid report ARN",
            "case {case:?}: expected CodeBuild's report-ARN sentence, got {stated:?}"
        );
        assert_eq!(
            stated,
            state::INVALID_REPORT_ARN_MESSAGE,
            "case {case:?}: the service must state one sentence for this class, got {stated:?}"
        );
        assert!(
            !stated.contains(value.as_str()) && !stated.contains("reportArns"),
            "case {case:?}: the sentence must quote neither the value nor the member, \
             got {stated:?}"
        );
    }

    // ONE malformed value refuses the whole call, whatever else the batch asks about:
    // the answer is positional, so a value that addresses no report at all cannot be
    // placed in either half. The report the store holds is not reported, and the store
    // is left exactly as it was.
    for (case, value) in &malformed {
        let batch = vec![held_arn.clone(), value.clone(), held_arn.clone()];
        let stated = refused(&router, &format!("{case}, mixed into a valid batch"), &batch).await;
        assert_eq!(
            stated,
            state::INVALID_REPORT_ARN_MESSAGE,
            "case {case:?}: a batch carrying one malformed value must be refused the same \
             way, got {stated:?}"
        );
    }
    let held_after = engine.state::<CodeBuildState>().lock().reports.len();
    assert_eq!(
        held_after, 1,
        "a refusal must leave the store as it was, got {held_after} record(s)"
    );

    // The control: the same batch without the malformed value is served.
    let read = batch_get(&router, &[held_arn.clone(), held_arn.clone()]).await;
    assert_eq!(
        read["reports"].as_array().map(Vec::len),
        Some(2),
        "a batch of well-formed ARNs must be served, got {read}"
    );
}

/// RULE: a store that holds no report answers every well-formed report ARN in
/// `reportsNotFound` — the reader reports what the store holds rather than refusing, so
/// an account with no reports is a successful empty answer.
#[tokio::test]
async fn an_empty_store_reports_every_arn_as_not_found() {
    let router = emulator();
    let arns = [
        state::report_arn(REGION, "absent-group", "0abc1234"),
        state::report_arn(REGION, "absent-group", "5def6789"),
    ];

    let read = batch_get(&router, &arns).await;
    assert_eq!(
        read["reports"],
        json!([]),
        "an empty store must report no reports, got {read}"
    );
    assert_eq!(
        read["reportsNotFound"],
        json!([arns[0], arns[1]]),
        "an empty store must report every ARN missing, in request order, got {read}"
    );
}
