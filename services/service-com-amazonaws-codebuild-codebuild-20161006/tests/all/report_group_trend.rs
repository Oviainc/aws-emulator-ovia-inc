// SPDX-License-Identifier: CC0-1.0

//! What `GetReportGroupTrend` accumulates for a report group, driven over the wire the
//! router serves: `POST /` with `X-Amz-Target: CodeBuild_20161006.GetReportGroupTrend`
//! on awsJson1_1.
//!
//! Four rules, each asserted over a table rather than over one request:
//!
//! * a trend reports one data point per analysed run of the group — the value of the
//!   modelled report-summary member `trendField` names — and `stats` is the smallest,
//!   the largest and the mean of those points;
//! * the runs analysed are the group's OWN, most recent first, cut to `numOfReports`
//!   (100 when the request omits it);
//! * a value that is not a report group ARN of this service is refused as input, and an
//!   ARN this account holds no report group under is refused as not found;
//! * a group holding no run the field applies to is a successful EMPTY trend, and every
//!   read leaves what a later caller sees unchanged.
//!
//! A report run is minted by a build whose buildspec declares one, so the runs here are
//! seeded straight into the store (as `tests/all/batch_get_reports.rs` seeds its own) —
//! what the trend answers can then only have come from the store.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_codebuild_codebuild_20161006::ops::types::{
    CodeCoverageReportSummary, Report, TestReportSummary, REPORT_GROUP_TREND_FIELD_TYPE_VALUES,
};
use service_com_amazonaws_codebuild_codebuild_20161006::state::{
    self, ComAmazonawsCodebuildCodebuild20161006State as CodeBuildState,
};
use smithy_server_runtime::MultiServiceRouter;

// The harness every suite in this test crate drives the service through: one router over
// an engine this test holds, and one awsJson1_1 call signed for [`REGION`].
use super::project_build_lifecycle::{call, emulator_on, error_type, REGION};
use super::report_group_lifecycle::create_report_group_body;

/// The report group whose runs are test reports.
const TEST_GROUP: &str = "unit-tests";

/// The report group whose runs are code coverage reports.
const COVERAGE_GROUP: &str = "coverage-suite";

/// A third group, whose runs no trend of the two above may count.
const OTHER_GROUP: &str = "other-tests";

/// The instant the OLDEST seeded run occurred, as epoch millis. Each later run is a whole
/// second after it, so the order a trend cuts its selection from is unambiguous.
const FIRST_RUN_MILLIS: i64 = 1_700_000_000_000;

/// When a seeded run reports it expires. AWS documents the span on `Report$expired`: "A
/// report expires 30 days after it is created."
const EXPIRY_MILLIS: i64 = 30 * 24 * 60 * 60 * 1_000;

/// The `statusCounts` key counting the test cases that passed. `TestCase$status`: "Valid
/// statuses are SUCCEEDED, FAILED, ERROR, SKIPPED, and UNKNOWN."
const PASSED: &str = "SUCCEEDED";

/// The `statusCounts` key counting the test cases that failed, from the same catalogue.
const FAILED: &str = "FAILED";

/// The ids of [`TEST_GROUP`]'s runs, MOST RECENT FIRST — the order a trend reports them.
const TEST_RUN_IDS: [&str; 3] = ["00000003", "00000002", "00000001"];

/// The ids of [`COVERAGE_GROUP`]'s runs, most recent first.
const COVERAGE_RUN_IDS: [&str; 3] = ["0000000c", "0000000b", "0000000a"];

/// The ARN a report group of this account carries, as the service's own minter builds it.
fn group_arn(name: &str) -> String {
    state::report_group_arn(REGION, name)
}

/// Create `name` over the wire, asserting it succeeded, and hand back the ARN the service
/// minted for it — so every trend below addresses a group this account really holds.
async fn create_group(router: &MultiServiceRouter, name: &str, report_type: &str) -> String {
    let body = create_report_group_body(
        name,
        report_type,
        json!({"exportConfigType": "NO_EXPORT"}),
        None,
    );
    let (status, response) = call(router, "CreateReportGroup", body).await;
    assert_eq!(
        status, 200,
        "CreateReportGroup {name:?}: expected 200, got {status} with {response}"
    );
    let arn = response["reportGroup"]["arn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert_eq!(
        arn,
        group_arn(name),
        "CreateReportGroup {name:?} must mint the service's own ARN, got {response}"
    );
    arn
}

/// Ask for a trend, asserting the call succeeded.
async fn get_trend(router: &MultiServiceRouter, body: Value) -> Value {
    let (status, response) = call(router, "GetReportGroupTrend", body.clone()).await;
    assert_eq!(
        status, 200,
        "GetReportGroupTrend {body}: expected 200, got {status} with {response}"
    );
    response
}

/// Ask for a trend, assert it was refused with HTTP 400, and hand back the error code and
/// the sentence it stated.
async fn refused(router: &MultiServiceRouter, case: &str, body: Value) -> (String, String) {
    let (status, response) = call(router, "GetReportGroupTrend", body.clone()).await;
    assert_eq!(
        status, 400,
        "case {case:?} ({body}): expected 400, got {status} with {response}"
    );
    (
        error_type(&response),
        response["message"].as_str().unwrap_or_default().to_string(),
    )
}

/// One report run of `group`, as the store holds one: its ARNs come from the service's own
/// minters, so the test cannot drift from the forms the handlers resolve. `order` places
/// the run on the clock — the higher it is, the more recent the run.
fn run(group: &str, id: &str, order: i64, report_type: &str) -> Report {
    let created = FIRST_RUN_MILLIS + order * 1_000;
    Report {
        arn: Some(state::report_arn(REGION, group, id)),
        r#type: Some(report_type.to_string()),
        name: Some(group.to_string()),
        report_group_arn: Some(group_arn(group)),
        execution_id: Some(state::build_arn(REGION, &format!("{group}:{id}"))),
        status: Some(PASSED.to_string()),
        created: Some(state::datetime(created)),
        expired: Some(state::datetime(created + EXPIRY_MILLIS)),
        export_config: None,
        truncated: Some(false),
        test_summary: None,
        code_coverage_summary: None,
    }
}

/// A TEST run of `group` whose summary reports `total` test cases, `passed` of which
/// passed, taking `duration_nanos` nanoseconds in all.
fn test_run(
    group: &str,
    id: &str,
    order: i64,
    total: i32,
    passed: i32,
    duration_nanos: i64,
) -> Report {
    let mut report = run(group, id, order, "TEST");
    let mut status_counts = HashMap::new();
    if passed > 0 {
        status_counts.insert(PASSED.to_string(), passed);
    }
    if total > passed {
        status_counts.insert(FAILED.to_string(), total - passed);
    }
    report.test_summary = Some(TestReportSummary {
        total,
        status_counts,
        duration_in_nano_seconds: duration_nanos,
    });
    report
}

/// A CODE_COVERAGE run of `group` carrying `summary`.
fn coverage_run(group: &str, id: &str, order: i64, summary: CodeCoverageReportSummary) -> Report {
    let mut report = run(group, id, order, "CODE_COVERAGE");
    report.code_coverage_summary = Some(summary);
    report
}

/// The three test runs [`TEST_GROUP`]'s trends are taken over, oldest first.
fn test_runs() -> Vec<Report> {
    vec![
        // total 4, 3 passed -> a 75% pass rate
        test_run(TEST_GROUP, "00000001", 1, 4, 3, 1_000_000_000),
        // total 2, both passed -> 100%
        test_run(TEST_GROUP, "00000002", 2, 2, 2, 3_000_000_000),
        // total 6, 3 passed -> 50%
        test_run(TEST_GROUP, "00000003", 3, 6, 3, 2_000_000_000),
    ]
}

/// The three coverage runs [`COVERAGE_GROUP`]'s trends are taken over, oldest first.
fn coverage_runs() -> Vec<Report> {
    vec![
        coverage_run(
            COVERAGE_GROUP,
            "0000000a",
            1,
            CodeCoverageReportSummary {
                line_coverage_percentage: Some(50.0),
                lines_covered: Some(10),
                lines_missed: Some(10),
                branch_coverage_percentage: Some(25.0),
                branches_covered: Some(2),
                branches_missed: Some(6),
            },
        ),
        coverage_run(
            COVERAGE_GROUP,
            "0000000b",
            2,
            CodeCoverageReportSummary {
                line_coverage_percentage: Some(75.0),
                lines_covered: Some(30),
                lines_missed: Some(8),
                branch_coverage_percentage: Some(50.0),
                branches_covered: Some(4),
                branches_missed: Some(4),
            },
        ),
        coverage_run(
            COVERAGE_GROUP,
            "0000000c",
            3,
            CodeCoverageReportSummary {
                line_coverage_percentage: Some(100.0),
                lines_covered: Some(20),
                lines_missed: Some(0),
                branch_coverage_percentage: Some(75.0),
                branches_covered: Some(6),
                branches_missed: Some(2),
            },
        ),
    ]
}

/// Write `runs` straight into the store, keyed by each run's own ARN — which is how every
/// report operation addresses one.
fn seed(engine: &Arc<engine::InMemoryEngine>, runs: Vec<Report>) {
    let handle = engine.state::<CodeBuildState>();
    let mut guard = handle.lock();
    for report in runs {
        let arn = report.arn.clone().expect("a seeded run carries its ARN");
        guard.reports.insert(arn, report);
    }
}

/// How many report runs the store holds, for asserting a read wrote nothing.
fn held_runs(engine: &Arc<engine::InMemoryEngine>) -> usize {
    engine.state::<CodeBuildState>().lock().reports.len()
}

/// The `rawData` a trend over `ids` of `group` must report, each id paired with its data.
fn expected_raw_data(group: &str, ids: &[&str], data: &[&str]) -> Value {
    Value::Array(
        ids.iter()
            .zip(data.iter())
            .map(|(id, point)| {
                json!({"reportArn": state::report_arn(REGION, group, id), "data": point})
            })
            .collect(),
    )
}

/// One row of the trend-field table: the `trendField` asked for, the group whose runs
/// answer it, the three data points expected (newest run first), and the smallest, the
/// largest and the mean of them.
type TrendCase = (
    &'static str,
    &'static str,
    [&'static str; 3],
    &'static str,
    &'static str,
    &'static str,
);

/// RULE: a trend reports one data point per analysed run of the group the request
/// addresses — the value of the modelled report-summary member `trendField` names, in
/// that member's own unit — newest run first, and `stats` is the smallest, the largest
/// and the mean of exactly those points.
///
/// The table is the model's WHOLE `trendField` catalogue
/// (`crate::ops::types::REPORT_GROUP_TREND_FIELD_TYPE_VALUES`, from
/// `smithy_query(query="shape",shape="ReportGroupTrendFieldType")`): the three test
/// fields over a TEST group and the six coverage fields over a CODE_COVERAGE group, with
/// counts, nanosecond durations and percentages among them, so each field is checked
/// against its own member rather than against one example. A third group's runs are in
/// the same store throughout and may appear in neither trend.
#[tokio::test]
async fn every_trend_field_accumulates_its_modelled_summary_member() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(Arc::clone(&engine));
    create_group(&router, TEST_GROUP, "TEST").await;
    create_group(&router, COVERAGE_GROUP, "CODE_COVERAGE").await;
    create_group(&router, OTHER_GROUP, "TEST").await;

    let mut runs = test_runs();
    runs.extend(coverage_runs());
    // The decoy: a run of another group, with values no trend below reports.
    runs.push(test_run(OTHER_GROUP, "0000ffff", 9, 99, 11, 9_000_000_000));
    seed(&engine, runs);

    // (trendField, group, data newest-first, min, max, average)
    let cases: Vec<TrendCase> = vec![
        // `testSummary.total`
        ("TOTAL", TEST_GROUP, ["6", "2", "4"], "2", "6", "4"),
        // `testSummary.durationInNanoSeconds`
        (
            "DURATION",
            TEST_GROUP,
            ["2000000000", "3000000000", "1000000000"],
            "1000000000",
            "3000000000",
            "2000000000",
        ),
        // "the percentage of tests that passed": the summary's passed count over its total
        ("PASS_RATE", TEST_GROUP, ["50", "100", "75"], "50", "100", "75"),
        // `codeCoverageSummary.lineCoveragePercentage`
        (
            "LINE_COVERAGE",
            COVERAGE_GROUP,
            ["100", "75", "50"],
            "50",
            "100",
            "75",
        ),
        // `codeCoverageSummary.linesCovered`
        (
            "LINES_COVERED",
            COVERAGE_GROUP,
            ["20", "30", "10"],
            "10",
            "30",
            "20",
        ),
        // `codeCoverageSummary.linesMissed` — a run that missed NOTHING contributes the
        // measured zero, which is not the same as contributing no point at all.
        (
            "LINES_MISSED",
            COVERAGE_GROUP,
            ["0", "8", "10"],
            "0",
            "10",
            "6",
        ),
        // `codeCoverageSummary.branchCoveragePercentage`
        (
            "BRANCH_COVERAGE",
            COVERAGE_GROUP,
            ["75", "50", "25"],
            "25",
            "75",
            "50",
        ),
        // `codeCoverageSummary.branchesCovered`
        (
            "BRANCHES_COVERED",
            COVERAGE_GROUP,
            ["6", "4", "2"],
            "2",
            "6",
            "4",
        ),
        // `codeCoverageSummary.branchesMissed`
        (
            "BRANCHES_MISSED",
            COVERAGE_GROUP,
            ["2", "4", "6"],
            "2",
            "6",
            "4",
        ),
    ];

    // The table spans the catalogue: every modelled value is a row, and no row names a
    // value the model does not declare.
    for field in REPORT_GROUP_TREND_FIELD_TYPE_VALUES {
        assert!(
            cases.iter().any(|(case, ..)| case == field),
            "trendField {field:?} is modelled but untested"
        );
    }
    assert_eq!(
        cases.len(),
        REPORT_GROUP_TREND_FIELD_TYPE_VALUES.len(),
        "the table must have exactly one row per modelled trendField, got {} row(s)",
        cases.len()
    );

    for (field, group, data, min, max, average) in &cases {
        let trend = get_trend(
            &router,
            json!({"reportGroupArn": group_arn(group), "trendField": field}),
        )
        .await;
        let ids = if *group == TEST_GROUP {
            TEST_RUN_IDS
        } else {
            COVERAGE_RUN_IDS
        };
        assert_eq!(
            trend["rawData"],
            expected_raw_data(group, &ids, data),
            "case {field:?}: expected one point per run of {group:?}, newest first, \
             got {trend}"
        );
        assert_eq!(
            trend["stats"],
            json!({"average": average, "max": max, "min": min}),
            "case {field:?}: expected the accumulation of {data:?}, got {trend}"
        );
    }

    // Purely reporting: nine trends later the store holds exactly the runs that were
    // seeded, so nothing a later caller sees was changed by reading.
    assert_eq!(
        held_runs(&engine),
        7,
        "a trend must write nothing, got {} run(s) held",
        held_runs(&engine)
    );
}

/// RULE: the runs a trend analyses are the group's own, MOST RECENT FIRST, cut to
/// `numOfReports` — "This operation always retrieves the most recent reports. If this
/// parameter is omitted, the most recent 100 reports are analyzed" — and a value outside
/// that member's modelled range is refused.
///
/// The rows are each count up to the number of runs held, the published maximum, and the
/// absent member; the refusals are one past each published bound of `PageSize`
/// (`@range(min: 1, max: 100)`) plus a negative.
#[tokio::test]
async fn a_trend_analyses_only_the_most_recent_reports_asked_for() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(Arc::clone(&engine));
    create_group(&router, TEST_GROUP, "TEST").await;
    seed(&engine, test_runs());

    // (case, numOfReports, the data expected, newest first)
    let cases: Vec<(&str, Option<i32>, Vec<&str>)> = vec![
        ("the most recent run alone", Some(1), vec!["6"]),
        ("the two most recent", Some(2), vec!["6", "2"]),
        ("every run held", Some(3), vec!["6", "2", "4"]),
        ("more than are held", Some(50), vec!["6", "2", "4"]),
        ("at the published maximum", Some(100), vec!["6", "2", "4"]),
        ("omitted, so the default 100", None, vec!["6", "2", "4"]),
    ];

    for (case, num_of_reports, data) in &cases {
        let mut body = json!({"reportGroupArn": group_arn(TEST_GROUP), "trendField": "TOTAL"});
        if let (Value::Object(members), Some(count)) = (&mut body, num_of_reports) {
            members.insert("numOfReports".to_string(), json!(count));
        }
        let trend = get_trend(&router, body).await;
        let ids: Vec<&str> = TEST_RUN_IDS.iter().take(data.len()).copied().collect();
        assert_eq!(
            trend["rawData"],
            expected_raw_data(TEST_GROUP, &ids, data),
            "case {case:?}: expected the {} most recent run(s), got {trend}",
            data.len()
        );
    }

    for (case, num_of_reports) in [
        ("one below the minimum", 0),
        ("one past the maximum", 101),
        ("negative", -1),
    ] {
        let (status, response) = call(
            &router,
            "GetReportGroupTrend",
            json!({
                "reportGroupArn": group_arn(TEST_GROUP),
                "trendField": "TOTAL",
                "numOfReports": num_of_reports,
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?} (numOfReports {num_of_reports}): expected 400, got {status} \
             with {response}"
        );
        assert!(
            error_type(&response).contains("InvalidInputException"),
            "case {case:?}: expected InvalidInputException, got {response}"
        );
    }
}

/// RULE: a report group is addressed by ARN alone, so a value that is not a report group
/// ARN of this service is refused as INPUT — 400 `InvalidInputException` worded
/// `Invalid report group ARN`, fixed text quoting neither the value nor the member —
/// while an ARN this account holds no group under is refused as NOT FOUND — 400
/// `ResourceNotFoundException` quoting the ARN it was asked about inside CodeBuild's
/// absent-report-group sentence.
///
/// The not-found table spans the ways a well-formed group ARN can address no held group
/// (never created, another partition, region or account, a different name, a different
/// case, and a group that was deleted); the input table spans the forms a value can be
/// malformed in (not an ARN, another service's ARN, another resource of this one, an
/// empty name, a truncated ARN, the resource segment alone).
#[tokio::test]
async fn an_arn_that_addresses_no_held_report_group_has_no_trend() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(Arc::clone(&engine));
    create_group(&router, TEST_GROUP, "TEST").await;
    create_group(&router, OTHER_GROUP, "TEST").await;
    seed(&engine, test_runs());
    let account = engine::EMULATOR_ACCOUNT_ID;

    // A group that existed and was deleted holds no trend either.
    let (status, response) = call(
        &router,
        "DeleteReportGroup",
        json!({"arn": group_arn(OTHER_GROUP)}),
    )
    .await;
    assert_eq!(
        status, 200,
        "DeleteReportGroup: expected 200, got {status} with {response}"
    );

    let absent: Vec<(&str, String)> = vec![
        ("a group never created", group_arn("never-created")),
        (
            "another partition",
            format!("arn:aws-cn:codebuild:{REGION}:{account}:report-group/{TEST_GROUP}"),
        ),
        (
            "another region",
            format!("arn:aws:codebuild:eu-west-1:{account}:report-group/{TEST_GROUP}"),
        ),
        (
            "another account",
            format!("arn:aws:codebuild:{REGION}:111122223333:report-group/{TEST_GROUP}"),
        ),
        ("the held group in another case", group_arn("Unit-Tests")),
        ("a group that was deleted", group_arn(OTHER_GROUP)),
    ];

    for (case, arn) in &absent {
        let (code, stated) = refused(
            &router,
            case,
            json!({"reportGroupArn": arn, "trendField": "TOTAL"}),
        )
        .await;
        assert_eq!(
            code, "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {code:?} / {stated:?}"
        );
        assert_eq!(
            stated,
            state::provided_report_group_absent_message(arn),
            "case {case:?}: expected CodeBuild's absent-report-group sentence, got {stated:?}"
        );
        assert!(
            stated.contains(arn.as_str()),
            "case {case:?}: the sentence must name the ARN asked about, got {stated:?}"
        );
    }

    let malformed: Vec<(&str, String)> = vec![
        ("a bare report group name", TEST_GROUP.to_string()),
        (
            "a project ARN of this service",
            format!("arn:aws:codebuild:{REGION}:{account}:project/{TEST_GROUP}"),
        ),
        (
            "a report ARN of this service",
            state::report_arn(REGION, TEST_GROUP, "00000001"),
        ),
        (
            "another service's ARN",
            format!("arn:aws:codepipeline:{REGION}:{account}:report-group/{TEST_GROUP}"),
        ),
        (
            "a report group resource with no name",
            format!("arn:aws:codebuild:{REGION}:{account}:report-group/"),
        ),
        (
            "a truncated ARN",
            format!("arn:aws:codebuild:{REGION}:{account}"),
        ),
        (
            "the resource segment alone",
            format!("report-group/{TEST_GROUP}"),
        ),
    ];

    for (case, value) in &malformed {
        let (code, stated) = refused(
            &router,
            case,
            json!({"reportGroupArn": value, "trendField": "TOTAL"}),
        )
        .await;
        assert_eq!(
            code, "InvalidInputException",
            "case {case:?}: expected InvalidInputException, got {code:?} / {stated:?}"
        );
        assert_eq!(
            stated,
            state::INVALID_REPORT_GROUP_ARN_MESSAGE,
            "case {case:?}: the service must state one sentence for this class, got {stated:?}"
        );
        assert!(
            !stated.contains(value.as_str()) && !stated.contains("reportGroupArn"),
            "case {case:?}: the sentence must quote neither the value nor the member, \
             got {stated:?}"
        );
    }

    // Every refusal left the store as it was, and the control still answers.
    assert_eq!(
        held_runs(&engine),
        3,
        "a refusal must write nothing, got {} run(s) held",
        held_runs(&engine)
    );
    let trend = get_trend(
        &router,
        json!({"reportGroupArn": group_arn(TEST_GROUP), "trendField": "TOTAL"}),
    )
    .await;
    assert_eq!(
        trend["rawData"],
        expected_raw_data(TEST_GROUP, &TEST_RUN_IDS, &["6", "2", "4"]),
        "the held group must still be served, got {trend}"
    );
}

/// RULE: a report group holding no run the requested field applies to is a successful
/// EMPTY trend — `rawData` an empty list and each of `stats`' three accumulations the
/// empty string, since nothing was measured — and reading one changes nothing a later
/// caller sees.
///
/// The rows span the ways a group can have nothing to accumulate: no run at all, only
/// runs of the other report kind (a test report carries no coverage summary and a
/// coverage report no test summary), a test report with no test cases at all asked for
/// its pass rate, and a group whose runs all belong to another group. The control in each
/// row is a field the same store DOES answer, so an empty trend is never an empty store.
#[tokio::test]
async fn a_group_with_no_run_for_the_field_reports_an_empty_trend() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(Arc::clone(&engine));
    create_group(&router, TEST_GROUP, "TEST").await;
    create_group(&router, COVERAGE_GROUP, "CODE_COVERAGE").await;
    create_group(&router, OTHER_GROUP, "TEST").await;

    let mut runs = test_runs();
    runs.extend(coverage_runs());
    // A test run that reports no test case at all: it has a total to accumulate, but no
    // share of its tests that passed.
    runs.push(test_run(OTHER_GROUP, "00000010", 4, 0, 0, 0));
    seed(&engine, runs);

    // (case, group, trendField)
    let empty: Vec<(&str, &str, &str)> = vec![
        ("a group holding no run at all", "trend-less", "TOTAL"),
        ("test runs asked for line coverage", TEST_GROUP, "LINE_COVERAGE"),
        (
            "test runs asked for branches missed",
            TEST_GROUP,
            "BRANCHES_MISSED",
        ),
        (
            "coverage runs asked for a total",
            COVERAGE_GROUP,
            "TOTAL",
        ),
        (
            "coverage runs asked for a duration",
            COVERAGE_GROUP,
            "DURATION",
        ),
        (
            "coverage runs asked for a pass rate",
            COVERAGE_GROUP,
            "PASS_RATE",
        ),
        (
            "a run with no test case asked for its pass rate",
            OTHER_GROUP,
            "PASS_RATE",
        ),
    ];
    create_group(&router, "trend-less", "TEST").await;

    for (case, group, field) in &empty {
        let trend = get_trend(
            &router,
            json!({"reportGroupArn": group_arn(group), "trendField": field}),
        )
        .await;
        assert_eq!(
            trend["rawData"],
            json!([]),
            "case {case:?}: expected no data point, got {trend}"
        );
        assert_eq!(
            trend["stats"],
            json!({"average": "", "max": "", "min": ""}),
            "case {case:?}: expected every accumulation of nothing stated as empty text, \
             got {trend}"
        );
    }

    // The controls: the same store answers the field each group's own runs DO carry, and
    // the run with no test case still reports the total it measured.
    for (case, group, field, data) in [
        ("the test group's totals", TEST_GROUP, "TOTAL", "6"),
        (
            "the coverage group's line coverage",
            COVERAGE_GROUP,
            "LINE_COVERAGE",
            "100",
        ),
        ("a run that reported no test case", OTHER_GROUP, "TOTAL", "0"),
    ] {
        let trend = get_trend(
            &router,
            json!({"reportGroupArn": group_arn(group), "trendField": field}),
        )
        .await;
        assert_eq!(
            trend["rawData"][0]["data"],
            json!(data),
            "case {case:?}: expected {data:?} from the most recent run, got {trend}"
        );
    }

    // A read leaves what a later caller sees unchanged: the same request answers the same
    // thing, over a store holding exactly the runs that were seeded.
    let before = get_trend(
        &router,
        json!({"reportGroupArn": group_arn(TEST_GROUP), "trendField": "PASS_RATE"}),
    )
    .await;
    let again = get_trend(
        &router,
        json!({"reportGroupArn": group_arn(TEST_GROUP), "trendField": "PASS_RATE"}),
    )
    .await;
    assert_eq!(
        before, again,
        "a repeated trend must answer the same thing, got {before} then {again}"
    );
    assert_eq!(
        held_runs(&engine),
        7,
        "a trend must write nothing, got {} run(s) held",
        held_runs(&engine)
    );
}
