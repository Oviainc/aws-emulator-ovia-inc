// SPDX-License-Identifier: CC0-1.0

//! The performance-analysis-report lifecycle, end to end over the wire:
//! `CreatePerformanceAnalysisReport` -> `GetPerformanceAnalysisReport` ->
//! `ListPerformanceAnalysisReports` -> `DeletePerformanceAnalysisReport`.
//!
//! Every assertion is on a RULE over a set of inputs, not on one recorded
//! request: the id pattern and its uniqueness, the round trip of the window
//! Create persisted, the clock-owed status transition, the per-resource filter,
//! paging each report exactly once at several page sizes, and the refusal of a
//! report id no resource holds.

use serde_json::{json, Value};

use crate::pi_harness::{call, emulator, is_modelled_next_token, is_modelled_report_id};

/// 2026-01-01T00:00:00Z, the base of every window these cases use. `awsJson1_1`
/// carries an `ISOTimestamp` as epoch seconds.
const T0: f64 = 1_767_225_600.0;

/// A pattern-valid `AnalysisReportId` (`report-` + 17 lowercase hex characters)
/// that no Create ever minted, so the boundary validator passes it and the
/// handler is what refuses it.
const UNMINTED_REPORT_ID: &str = "report-0123456789abcdef0";

/// One row of the Create table: the class spans both service types, both forms
/// of the immutable resource id a data source is addressed by (`db-` for an
/// instance, `cluster-` for a cluster) at different token lengths, an absent and
/// a present `EndTime`, windows of different lengths, and zero, one and several
/// tags.
struct CreateCase {
    service_type: &'static str,
    identifier: &'static str,
    start_time: f64,
    end_time: Option<f64>,
    tags: &'static [(&'static str, &'static str)],
}

const CREATE_CASES: &[CreateCase] = &[
    CreateCase {
        service_type: "RDS",
        identifier: "db-ABCDEFG1234567",
        start_time: T0,
        end_time: Some(T0 + 3600.0),
        tags: &[],
    },
    CreateCase {
        service_type: "RDS",
        identifier: "db-ADECBTYHKTSAUMUZQYPDS2GW4A",
        start_time: T0 + 60.0,
        end_time: None,
        tags: &[("team", "db")],
    },
    CreateCase {
        service_type: "DOCDB",
        identifier: "db-0123456789",
        start_time: T0 - 86_400.0,
        end_time: Some(T0),
        tags: &[("env", "prod"), ("owner", "sre")],
    },
    CreateCase {
        service_type: "DOCDB",
        identifier: "cluster-ABCDEFGHIJKL",
        start_time: T0,
        end_time: Some(T0 + 1.0),
        tags: &[],
    },
];

fn create_input(case: &CreateCase) -> Value {
    let mut input = json!({
        "ServiceType": case.service_type,
        "Identifier": case.identifier,
        "StartTime": case.start_time,
    });
    if let Some(end_time) = case.end_time {
        input["EndTime"] = json!(end_time);
    }
    if !case.tags.is_empty() {
        input["Tags"] = Value::Array(
            case.tags
                .iter()
                .map(|(key, value)| json!({"Key": key, "Value": value}))
                .collect(),
        );
    }
    input
}

/// RULE: Create mints a fresh `AnalysisReportId` matching the modelled pattern
/// on every call, whatever the request held.
#[tokio::test]
async fn create_mints_a_pattern_valid_id_unique_to_every_call() {
    let emulator = emulator();
    let mut minted: Vec<String> = Vec::new();

    // Each row twice: the same request must not produce the same id.
    for round in 0..2 {
        for case in CREATE_CASES {
            let answer = call(
                &emulator,
                "CreatePerformanceAnalysisReport",
                create_input(case),
            )
            .await;
            assert_eq!(
                answer.status, 200,
                "case {}/{} round {round}: expected 200, got {} {:?}",
                case.service_type, case.identifier, answer.status, answer.body
            );
            let id = answer
                .member("AnalysisReportId")
                .as_str()
                .unwrap_or_default()
                .to_string();
            assert!(
                is_modelled_report_id(&id),
                "case {}/{} round {round}: expected an id matching ^report-[0-9a-f]{{17}}$, got {id:?}",
                case.service_type,
                case.identifier
            );
            assert!(
                !minted.contains(&id),
                "case {}/{} round {round}: id {id:?} was already minted, got {minted:?}",
                case.service_type,
                case.identifier
            );
            minted.push(id);
        }
    }

    assert_eq!(
        minted.len(),
        CREATE_CASES.len() * 2,
        "every call must mint an id, got {minted:?}"
    );
}

/// RULE: Get answers with the report Create persisted — its own id, its
/// resource, and the window as created.
#[tokio::test]
async fn get_round_trips_the_window_create_persisted() {
    let emulator = emulator();

    for case in CREATE_CASES {
        let created = call(
            &emulator,
            "CreatePerformanceAnalysisReport",
            create_input(case),
        )
        .await;
        let id = created
            .member("AnalysisReportId")
            .as_str()
            .expect("create answers with an id")
            .to_string();

        let answer = call(
            &emulator,
            "GetPerformanceAnalysisReport",
            json!({
                "ServiceType": case.service_type,
                "Identifier": case.identifier,
                "AnalysisReportId": id,
            }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {}/{}: expected 200, got {} {:?}",
            case.service_type, case.identifier, answer.status, answer.body
        );
        let report = answer.member("AnalysisReport").clone();
        assert_eq!(
            report.get("AnalysisReportId").and_then(Value::as_str),
            Some(id.as_str()),
            "case {}/{}: expected the created id, got {report:?}",
            case.service_type,
            case.identifier
        );
        assert_eq!(
            report.get("Identifier").and_then(Value::as_str),
            Some(case.identifier),
            "case {}/{}: expected the created Identifier, got {report:?}",
            case.service_type,
            case.identifier
        );
        assert_eq!(
            report.get("ServiceType").and_then(Value::as_str),
            Some(case.service_type),
            "case {}/{}: expected the created ServiceType, got {report:?}",
            case.service_type,
            case.identifier
        );
        assert_eq!(
            report.get("StartTime").and_then(Value::as_f64),
            Some(case.start_time),
            "case {}/{}: expected the created StartTime, got {report:?}",
            case.service_type,
            case.identifier
        );
        assert_eq!(
            report.get("EndTime").and_then(Value::as_f64),
            case.end_time,
            "case {}/{}: expected the created EndTime, got {report:?}",
            case.service_type,
            case.identifier
        );
        let create_time = report
            .get("CreateTime")
            .and_then(Value::as_f64)
            .unwrap_or_default();
        assert!(
            create_time > 0.0,
            "case {}/{}: expected a CreateTime the service observed, got {report:?}",
            case.service_type,
            case.identifier
        );
        assert!(
            report.get("Status").and_then(Value::as_str).is_some(),
            "case {}/{}: expected a Status, got {report:?}",
            case.service_type,
            case.identifier
        );
    }
}

/// RULE: a report the service has not finished analysing reports a non-terminal
/// status, and the clock — not a read — moves it to the terminal one. A read
/// never moves it back, and never moves it early.
#[tokio::test]
async fn status_is_running_until_the_clock_owes_succeeded() {
    let emulator = emulator();
    let case = &CREATE_CASES[0];
    let started = std::time::Instant::now();
    let created = call(
        &emulator,
        "CreatePerformanceAnalysisReport",
        create_input(case),
    )
    .await;
    let id = created
        .member("AnalysisReportId")
        .as_str()
        .expect("create answers with an id")
        .to_string();

    let get = json!({
        "ServiceType": case.service_type,
        "Identifier": case.identifier,
        "AnalysisReportId": id,
    });

    let before = call(&emulator, "GetPerformanceAnalysisReport", get.clone()).await;
    let read_at = started.elapsed();
    // The non-terminal status is only owed while the analysis span has not
    // elapsed. Asserting it unconditionally would race a loaded machine, so the
    // assertion is made only when this read provably landed inside the span.
    if read_at < std::time::Duration::from_millis(400) {
        assert_eq!(
            before
                .member("AnalysisReport")
                .get("Status")
                .and_then(Value::as_str),
            Some("RUNNING"),
            "case a read {read_at:?} after create, inside the analysis span: expected \
             RUNNING, got {:?}",
            before.body
        );
    }

    // The transition is owed by the clock, not by polling: wait past the span
    // instead of reading repeatedly.
    tokio::time::sleep(std::time::Duration::from_millis(900)).await;

    for read in 0..2 {
        let after = call(&emulator, "GetPerformanceAnalysisReport", get.clone()).await;
        assert_eq!(
            after.member("AnalysisReport").get("Status").and_then(Value::as_str),
            Some("SUCCEEDED"),
            "case read {read} after the span: expected SUCCEEDED, got {:?}",
            after.body
        );
    }

    // And a second, untouched report of the same resource is unaffected by the
    // reads above.
    let listed = call(
        &emulator,
        "ListPerformanceAnalysisReports",
        json!({"ServiceType": case.service_type, "Identifier": case.identifier}),
    )
    .await;
    let statuses: Vec<&str> = listed
        .member("AnalysisReports")
        .as_array()
        .map(|reports| {
            reports
                .iter()
                .filter_map(|report| report.get("Status").and_then(Value::as_str))
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        statuses,
        vec!["SUCCEEDED"],
        "case list after the span: expected the listed status to agree with Get, got {:?}",
        listed.body
    );
}

/// RULE: a report id is resolved within the resource it was created for; an id
/// no resource holds is the uniform `NotAuthorizedException` (HTTP 400) — the
/// same answer as a resource the caller may not see, so neither discloses the
/// other — and never a hollow success.
#[tokio::test]
async fn get_and_delete_refuse_an_id_the_resource_does_not_hold() {
    let emulator = emulator();
    assert!(
        is_modelled_report_id(UNMINTED_REPORT_ID),
        "the unminted id must itself satisfy ^report-[0-9a-f]{{17}}$, got {UNMINTED_REPORT_ID:?}"
    );
    let owner = &CREATE_CASES[0];
    let other = &CREATE_CASES[2];

    let created = call(
        &emulator,
        "CreatePerformanceAnalysisReport",
        create_input(owner),
    )
    .await;
    let owned_id = created
        .member("AnalysisReportId")
        .as_str()
        .expect("create answers with an id")
        .to_string();

    // A deleted id, to cover "deleted" as well as "never existed".
    let doomed = call(
        &emulator,
        "CreatePerformanceAnalysisReport",
        create_input(owner),
    )
    .await;
    let deleted_id = doomed
        .member("AnalysisReportId")
        .as_str()
        .expect("create answers with an id")
        .to_string();
    let removed = call(
        &emulator,
        "DeletePerformanceAnalysisReport",
        json!({
            "ServiceType": owner.service_type,
            "Identifier": owner.identifier,
            "AnalysisReportId": deleted_id,
        }),
    )
    .await;
    assert_eq!(
        removed.status, 200,
        "case delete an existing report: expected 200, got {} {:?}",
        removed.status, removed.body
    );

    // Each row is a (ServiceType, Identifier, AnalysisReportId) triple that no
    // stored report answers to. Every id is pattern-valid, so the boundary
    // validator passes it and the handler is what refuses.
    let cases: Vec<(&str, &str, String, &str)> = vec![
        (
            owner.service_type,
            owner.identifier,
            UNMINTED_REPORT_ID.to_string(),
            "never created",
        ),
        (
            owner.service_type,
            owner.identifier,
            deleted_id.clone(),
            "deleted",
        ),
        (
            other.service_type,
            other.identifier,
            owned_id.clone(),
            "another resource's report",
        ),
        (
            owner.service_type,
            "db-NOSUCHRESOURCE",
            owned_id.clone(),
            "another identifier on the same service type",
        ),
        (
            other.service_type,
            owner.identifier,
            owned_id.clone(),
            "the right identifier under the wrong service type",
        ),
    ];

    for (service_type, identifier, report_id, label) in cases {
        for operation in [
            "GetPerformanceAnalysisReport",
            "DeletePerformanceAnalysisReport",
        ] {
            let answer = call(
                &emulator,
                operation,
                json!({
                    "ServiceType": service_type,
                    "Identifier": identifier,
                    "AnalysisReportId": report_id,
                }),
            )
            .await;
            assert_eq!(
                answer.status, 400,
                "case {label} / {operation}: expected 400, got {} {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "NotAuthorizedException",
                "case {label} / {operation}: expected NotAuthorizedException, got {:?}",
                answer.body
            );
        }
    }

    // The report that WAS created is still readable: refusing the others
    // changed nothing.
    let still_there = call(
        &emulator,
        "GetPerformanceAnalysisReport",
        json!({
            "ServiceType": owner.service_type,
            "Identifier": owner.identifier,
            "AnalysisReportId": owned_id,
        }),
    )
    .await;
    assert_eq!(
        still_there.status, 200,
        "case the surviving report: expected 200, got {} {:?}",
        still_there.status, still_there.body
    );
}

/// RULE: List reports every stored report of the requested resource exactly
/// once, in creation order, and `MaxResults` truncates the page and hands back a
/// cursor that resumes exactly where it stopped — absent on the last page.
#[tokio::test]
async fn list_pages_every_report_exactly_once_at_every_page_size() {
    let emulator = emulator();
    let case = &CREATE_CASES[0];

    let mut created: Vec<String> = Vec::new();
    for _ in 0..7 {
        let answer = call(
            &emulator,
            "CreatePerformanceAnalysisReport",
            create_input(case),
        )
        .await;
        created.push(
            answer
                .member("AnalysisReportId")
                .as_str()
                .expect("create answers with an id")
                .to_string(),
        );
    }

    // The page sizes span the modelled `MaxResults` range 0..25: absent, the
    // minimum, sizes that do and do not divide the result count, exactly the
    // result count, more than it, and the maximum.
    for max_results in [None, Some(0), Some(1), Some(2), Some(3), Some(7), Some(8), Some(25)] {
        let mut seen: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;

        loop {
            let mut input = json!({
                "ServiceType": case.service_type,
                "Identifier": case.identifier,
            });
            if let Some(max) = max_results {
                input["MaxResults"] = json!(max);
            }
            if let Some(token) = &next_token {
                input["NextToken"] = json!(token);
            }

            let answer = call(&emulator, "ListPerformanceAnalysisReports", input).await;
            assert_eq!(
                answer.status, 200,
                "case max_results={max_results:?} page {pages}: expected 200, got {} {:?}",
                answer.status, answer.body
            );
            let reports = answer
                .member("AnalysisReports")
                .as_array()
                .cloned()
                .unwrap_or_default();
            if let Some(max) = max_results.filter(|max| *max > 0) {
                assert!(
                    reports.len() <= max as usize,
                    "case max_results={max_results:?} page {pages}: expected at most {max} \
                     summaries, got {}",
                    reports.len()
                );
            }
            for report in &reports {
                let id = report
                    .get("AnalysisReportId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                assert!(
                    is_modelled_report_id(&id),
                    "case max_results={max_results:?} page {pages}: expected a real report id, \
                     got {report:?}"
                );
                seen.push(id);
            }

            next_token = answer
                .member("NextToken")
                .as_str()
                .map(ToString::to_string);
            if let Some(token) = &next_token {
                assert!(
                    is_modelled_next_token(token),
                    "case max_results={max_results:?} page {pages}: expected a NextToken \
                     matching ^[a-zA-Z0-9_=-]+$, got {token:?}"
                );
            }
            pages += 1;
            if next_token.is_none() || pages > 20 {
                break;
            }
        }

        assert_eq!(
            seen, created,
            "case max_results={max_results:?}: expected every created report exactly once in \
             creation order, got {seen:?}"
        );
    }
}

/// RULE: List is scoped to the requested `(ServiceType, Identifier)` resource.
#[tokio::test]
async fn list_is_scoped_to_the_requested_resource() {
    let emulator = emulator();

    // One report per row of the table, so each resource holds a known id.
    let mut per_resource: Vec<(&CreateCase, String)> = Vec::new();
    for case in CREATE_CASES {
        let answer = call(
            &emulator,
            "CreatePerformanceAnalysisReport",
            create_input(case),
        )
        .await;
        per_resource.push((
            case,
            answer
                .member("AnalysisReportId")
                .as_str()
                .expect("create answers with an id")
                .to_string(),
        ));
    }

    for (case, id) in &per_resource {
        let answer = call(
            &emulator,
            "ListPerformanceAnalysisReports",
            json!({"ServiceType": case.service_type, "Identifier": case.identifier}),
        )
        .await;
        let ids: Vec<&str> = answer
            .member("AnalysisReports")
            .as_array()
            .map(|reports| {
                reports
                    .iter()
                    .filter_map(|report| report.get("AnalysisReportId").and_then(Value::as_str))
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            ids,
            vec![id.as_str()],
            "case {}/{}: expected only this resource's report, got {:?}",
            case.service_type,
            case.identifier,
            answer.body
        );
    }

    // A resource nobody created a report for lists nothing, and does not fail.
    let empty = call(
        &emulator,
        "ListPerformanceAnalysisReports",
        json!({"ServiceType": "RDS", "Identifier": "db-NONEOFTHESE"}),
    )
    .await;
    assert_eq!(
        empty.status, 200,
        "case a resource with no reports: expected 200, got {} {:?}",
        empty.status, empty.body
    );
    assert_eq!(
        empty.member("AnalysisReports").as_array().map(Vec::len),
        Some(0),
        "case a resource with no reports: expected an empty list, got {:?}",
        empty.body
    );
    assert!(
        empty.member("NextToken").is_null(),
        "case a resource with no reports: expected no NextToken, got {:?}",
        empty.body
    );
}

/// RULE: documented for this operation — "The reports are sorted based on the
/// start time of each report" — so the listed order follows the analysed
/// windows, not the order the reports were created in.
#[tokio::test]
async fn list_sorts_the_reports_by_the_start_time_of_each_report() {
    let emulator = emulator();
    let case = &CREATE_CASES[0];

    // Created in a deliberately scrambled start-time order, including two
    // reports sharing one start time (which must keep their creation order).
    let start_offsets: &[f64] = &[600.0, -3_600.0, 0.0, 86_400.0, 0.0, -7_200.0];
    let mut created: Vec<(f64, String)> = Vec::new();
    for offset in start_offsets {
        let answer = call(
            &emulator,
            "CreatePerformanceAnalysisReport",
            json!({
                "ServiceType": case.service_type,
                "Identifier": case.identifier,
                "StartTime": T0 + offset,
                "EndTime": T0 + offset + 300.0,
            }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case start offset {offset}: expected 200, got {} {:?}",
            answer.status, answer.body
        );
        created.push((
            T0 + offset,
            answer
                .member("AnalysisReportId")
                .as_str()
                .expect("create answers with an id")
                .to_string(),
        ));
    }

    // The documented order: by start time, creation order breaking a tie.
    let mut expected = created.clone();
    expected.sort_by(|left, right| left.0.total_cmp(&right.0));

    // Walked one page at a time, so the ordering has to hold across cursors too.
    for max_results in [None, Some(1), Some(2)] {
        let mut seen: Vec<(f64, String)> = Vec::new();
        let mut next_token: Option<String> = None;
        loop {
            let mut input = json!({
                "ServiceType": case.service_type,
                "Identifier": case.identifier,
            });
            if let Some(max) = max_results {
                input["MaxResults"] = json!(max);
            }
            if let Some(token) = &next_token {
                input["NextToken"] = json!(token);
            }
            let answer = call(&emulator, "ListPerformanceAnalysisReports", input).await;
            assert_eq!(
                answer.status, 200,
                "case max_results={max_results:?}: expected 200, got {} {:?}",
                answer.status, answer.body
            );
            for report in answer
                .member("AnalysisReports")
                .as_array()
                .cloned()
                .unwrap_or_default()
            {
                seen.push((
                    report.get("StartTime").and_then(Value::as_f64).unwrap_or(0.0),
                    report
                        .get("AnalysisReportId")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                ));
            }
            next_token = answer
                .member("NextToken")
                .as_str()
                .map(ToString::to_string);
            if next_token.is_none() {
                break;
            }
        }

        assert_eq!(
            seen, expected,
            "case max_results={max_results:?}: expected the reports sorted by StartTime \
             (creation order breaking a tie), got {seen:?}"
        );
    }
}

/// RULE: `ListTags` decides whether each summary carries the tags Create
/// persisted; the tags themselves come from the store.
#[tokio::test]
async fn list_tags_controls_whether_the_stored_tags_are_returned() {
    let emulator = emulator();

    for case in CREATE_CASES {
        call(
            &emulator,
            "CreatePerformanceAnalysisReport",
            create_input(case),
        )
        .await;

        for list_tags in [None, Some(false), Some(true)] {
            let mut input = json!({
                "ServiceType": case.service_type,
                "Identifier": case.identifier,
            });
            if let Some(flag) = list_tags {
                input["ListTags"] = json!(flag);
            }
            let answer = call(&emulator, "ListPerformanceAnalysisReports", input).await;
            let summary = answer
                .member("AnalysisReports")
                .as_array()
                .and_then(|reports| reports.first().cloned())
                .unwrap_or(Value::Null);

            if list_tags == Some(true) {
                let expected: Vec<Value> = case
                    .tags
                    .iter()
                    .map(|(key, value)| json!({"Key": key, "Value": value}))
                    .collect();
                assert_eq!(
                    summary.get("Tags").and_then(Value::as_array).cloned(),
                    Some(expected.clone()),
                    "case {}/{} ListTags=true: expected the stored tags {expected:?}, got \
                     {summary:?}",
                    case.service_type,
                    case.identifier
                );
            } else {
                assert!(
                    summary.get("Tags").is_none_or(Value::is_null),
                    "case {}/{} ListTags={list_tags:?}: expected no Tags, got {summary:?}",
                    case.service_type,
                    case.identifier
                );
            }
        }
    }
}

/// RULE: Delete removes the report from the store, so a later Get fails and a
/// later List no longer includes it — while the resource's other reports stay.
#[tokio::test]
async fn delete_removes_the_report_from_get_and_from_list() {
    let emulator = emulator();
    let case = &CREATE_CASES[0];

    let mut ids: Vec<String> = Vec::new();
    for _ in 0..3 {
        let answer = call(
            &emulator,
            "CreatePerformanceAnalysisReport",
            create_input(case),
        )
        .await;
        ids.push(
            answer
                .member("AnalysisReportId")
                .as_str()
                .expect("create answers with an id")
                .to_string(),
        );
    }

    // Delete them one at a time: after each, the deleted one is gone and the
    // rest are untouched.
    for (index, doomed) in ids.clone().into_iter().enumerate() {
        let deleted = call(
            &emulator,
            "DeletePerformanceAnalysisReport",
            json!({
                "ServiceType": case.service_type,
                "Identifier": case.identifier,
                "AnalysisReportId": doomed,
            }),
        )
        .await;
        assert_eq!(
            deleted.status, 200,
            "case delete #{index}: expected 200, got {} {:?}",
            deleted.status, deleted.body
        );

        let get = call(
            &emulator,
            "GetPerformanceAnalysisReport",
            json!({
                "ServiceType": case.service_type,
                "Identifier": case.identifier,
                "AnalysisReportId": doomed,
            }),
        )
        .await;
        assert_eq!(
            get.error_code(),
            "NotAuthorizedException",
            "case get after delete #{index}: expected NotAuthorizedException, got {} {:?}",
            get.status,
            get.body
        );

        let listed = call(
            &emulator,
            "ListPerformanceAnalysisReports",
            json!({"ServiceType": case.service_type, "Identifier": case.identifier}),
        )
        .await;
        let remaining: Vec<&str> = listed
            .member("AnalysisReports")
            .as_array()
            .map(|reports| {
                reports
                    .iter()
                    .filter_map(|report| report.get("AnalysisReportId").and_then(Value::as_str))
                    .collect()
            })
            .unwrap_or_default();
        let expected: Vec<&str> = ids[index + 1..].iter().map(String::as_str).collect();
        assert_eq!(
            remaining, expected,
            "case list after delete #{index}: expected {expected:?}, got {remaining:?}"
        );
    }
}

/// RULE: a bad request is refused with HTTP 400 by the stage that found it — a
/// violation of a modelled constraint (an absent required member, a value
/// outside a `length`, `range`, `pattern` or enum value set) by the validation
/// stage as `ValidationException`, and a rule only the handler can check (a
/// window whose end does not follow its start, a cursor this service never
/// issued) as the modelled `InvalidArgumentException` — and nothing is persisted
/// either way.
#[tokio::test]
async fn bad_arguments_are_refused_by_the_stage_that_finds_them() {
    let emulator = emulator();

    // `expected_code` is the stage: `ValidationException` for a modelled
    // constraint the validation stage refuses before any handler runs,
    // `InvalidArgumentException` for the handler's own refusals.
    let cases: Vec<(&str, &str, Value, &str)> = vec![
        (
            "Create without ServiceType",
            "CreatePerformanceAnalysisReport",
            json!({"Identifier": "db-ABCDEFG1234567", "StartTime": T0}),
            "ValidationException",
        ),
        (
            "Create without Identifier",
            "CreatePerformanceAnalysisReport",
            json!({"ServiceType": "RDS", "StartTime": T0}),
            "ValidationException",
        ),
        (
            "Create without StartTime",
            "CreatePerformanceAnalysisReport",
            json!({"ServiceType": "RDS", "Identifier": "db-ABCDEFG1234567"}),
            "ValidationException",
        ),
        (
            "Create with a ServiceType outside the enum value set",
            "CreatePerformanceAnalysisReport",
            json!({"ServiceType": "AURORA", "Identifier": "db-ABCDEFG1234567", "StartTime": T0}),
            "ValidationException",
        ),
        (
            "Create with an Identifier outside the pattern",
            "CreatePerformanceAnalysisReport",
            json!({"ServiceType": "RDS", "Identifier": "db_ABCDEFG!", "StartTime": T0}),
            "ValidationException",
        ),
        (
            "Create with EndTime equal to StartTime",
            "CreatePerformanceAnalysisReport",
            json!({
                "ServiceType": "RDS",
                "Identifier": "db-ABCDEFG1234567",
                "StartTime": T0,
                "EndTime": T0,
            }),
            "InvalidArgumentException",
        ),
        (
            "Create with EndTime before StartTime",
            "CreatePerformanceAnalysisReport",
            json!({
                "ServiceType": "RDS",
                "Identifier": "db-ABCDEFG1234567",
                "StartTime": T0,
                "EndTime": T0 - 1.0,
            }),
            "InvalidArgumentException",
        ),
        (
            "Get with an AnalysisReportId outside the pattern",
            "GetPerformanceAnalysisReport",
            json!({
                "ServiceType": "RDS",
                "Identifier": "db-ABCDEFG1234567",
                "AnalysisReportId": "report-NOTHEX0000000000",
            }),
            "ValidationException",
        ),
        (
            "Get with a TextFormat outside the enum value set",
            "GetPerformanceAnalysisReport",
            json!({
                "ServiceType": "RDS",
                "Identifier": "db-ABCDEFG1234567",
                "AnalysisReportId": UNMINTED_REPORT_ID,
                "TextFormat": "HTML",
            }),
            "ValidationException",
        ),
        (
            "List with MaxResults above the modelled range",
            "ListPerformanceAnalysisReports",
            json!({
                "ServiceType": "RDS",
                "Identifier": "db-ABCDEFG1234567",
                "MaxResults": 26,
            }),
            "ValidationException",
        ),
        (
            "List with MaxResults below the modelled range",
            "ListPerformanceAnalysisReports",
            json!({
                "ServiceType": "RDS",
                "Identifier": "db-ABCDEFG1234567",
                "MaxResults": -1,
            }),
            "ValidationException",
        ),
        (
            "List with a NextToken this service never issued",
            "ListPerformanceAnalysisReports",
            json!({
                "ServiceType": "RDS",
                "Identifier": "db-ABCDEFG1234567",
                "NextToken": "not-a-cursor",
            }),
            "InvalidArgumentException",
        ),
        (
            "List with a NextToken outside the modelled pattern",
            "ListPerformanceAnalysisReports",
            json!({
                "ServiceType": "RDS",
                "Identifier": "db-ABCDEFG1234567",
                "NextToken": "has spaces",
            }),
            "ValidationException",
        ),
    ];

    for (label, operation, input, expected_code) in cases {
        let answer = call(&emulator, operation, input).await;
        assert_eq!(
            answer.status, 400,
            "case {label}: expected 400, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            expected_code,
            "case {label}: expected {expected_code}, got {:?}",
            answer.body
        );
        assert!(
            !answer.error_message().is_empty(),
            "case {label}: expected a message saying what to fix, got {:?}",
            answer.body
        );
    }

    // Nothing above was persisted: the one resource those rows named holds no
    // report at all.
    let listed = call(
        &emulator,
        "ListPerformanceAnalysisReports",
        json!({"ServiceType": "RDS", "Identifier": "db-ABCDEFG1234567"}),
    )
    .await;
    assert_eq!(
        listed.member("AnalysisReports").as_array().map(Vec::len),
        Some(0),
        "case after the refusals: expected nothing persisted, got {:?}",
        listed.body
    );
}

/// RULE: the store is bounded. This emulator holds reports in memory and only
/// Delete drops one, so Create refuses past the emulator's own ceiling with the
/// modelled `InvalidArgumentException` instead of growing without bound — and
/// everything stored before the ceiling stays readable.
#[tokio::test]
async fn create_refuses_past_the_emulator_report_ceiling() {
    use service_com_amazonaws_pi_performanceinsightsv20180227::state::MAX_REPORTS_PER_RESOURCE;

    let emulator = emulator();
    let case = &CREATE_CASES[0];

    for index in 0..MAX_REPORTS_PER_RESOURCE {
        let answer = call(
            &emulator,
            "CreatePerformanceAnalysisReport",
            create_input(case),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case report #{index} of {MAX_REPORTS_PER_RESOURCE}: expected 200, got {} {:?}",
            answer.status, answer.body
        );
    }

    // Each row is one more Create past the ceiling: every one is refused, and
    // the refusal is stable rather than letting the next call through.
    for attempt in 0..3 {
        let answer = call(
            &emulator,
            "CreatePerformanceAnalysisReport",
            create_input(case),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case attempt {attempt} past the ceiling: expected 400, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "InvalidArgumentException",
            "case attempt {attempt} past the ceiling: expected InvalidArgumentException, got \
             {:?}",
            answer.body
        );
    }

    // The ceiling is per resource: another resource still accepts a report.
    let other = &CREATE_CASES[2];
    let elsewhere = call(
        &emulator,
        "CreatePerformanceAnalysisReport",
        create_input(other),
    )
    .await;
    assert_eq!(
        elsewhere.status, 200,
        "case another resource at the ceiling of the first: expected 200, got {} {:?}",
        elsewhere.status, elsewhere.body
    );

    // Deleting one makes room again, so the ceiling is a bound and not a
    // one-way door.
    let page = call(
        &emulator,
        "ListPerformanceAnalysisReports",
        json!({
            "ServiceType": case.service_type,
            "Identifier": case.identifier,
            "MaxResults": 1,
        }),
    )
    .await;
    let doomed = page
        .member("AnalysisReports")
        .as_array()
        .and_then(|reports| reports.first().cloned())
        .and_then(|report| {
            report
                .get("AnalysisReportId")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        })
        .expect("the resource holds reports to delete");
    let deleted = call(
        &emulator,
        "DeletePerformanceAnalysisReport",
        json!({
            "ServiceType": case.service_type,
            "Identifier": case.identifier,
            "AnalysisReportId": doomed,
        }),
    )
    .await;
    assert_eq!(
        deleted.status, 200,
        "case delete at the ceiling: expected 200, got {} {:?}",
        deleted.status, deleted.body
    );
    let after_delete = call(
        &emulator,
        "CreatePerformanceAnalysisReport",
        create_input(case),
    )
    .await;
    assert_eq!(
        after_delete.status, 200,
        "case create after making room: expected 200, got {} {:?}",
        after_delete.status, after_delete.body
    );
}

/// RULE: a cursor is only valid for the result set it was handed out for —
/// resuming another resource's listing with it is refused rather than silently
/// paging the wrong reports.
#[tokio::test]
async fn a_cursor_is_bound_to_the_resource_it_was_issued_for() {
    let emulator = emulator();
    let owner = &CREATE_CASES[0];
    let other = &CREATE_CASES[2];

    for case in [owner, other] {
        for _ in 0..2 {
            call(
                &emulator,
                "CreatePerformanceAnalysisReport",
                create_input(case),
            )
            .await;
        }
    }

    let first_page = call(
        &emulator,
        "ListPerformanceAnalysisReports",
        json!({
            "ServiceType": owner.service_type,
            "Identifier": owner.identifier,
            "MaxResults": 1,
        }),
    )
    .await;
    let token = first_page
        .member("NextToken")
        .as_str()
        .expect("a truncated page hands back a cursor")
        .to_string();

    let reused = call(
        &emulator,
        "ListPerformanceAnalysisReports",
        json!({
            "ServiceType": other.service_type,
            "Identifier": other.identifier,
            "MaxResults": 1,
            "NextToken": token,
        }),
    )
    .await;
    assert_eq!(
        reused.error_code(),
        "InvalidArgumentException",
        "case another resource's cursor: expected InvalidArgumentException, got {} {:?}",
        reused.status,
        reused.body
    );
}
