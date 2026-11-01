// SPDX-License-Identifier: CC0-1.0

//! Two members a response must not invent a value for, end to end over the
//! wire.
//!
//! * `SamplingTargetDocument$Interval` is "the number of seconds for the service
//!   to wait before getting sampling targets again": the cadence the SHARE of a
//!   rule's reservoir that the document grants comes with — come back for the
//!   next share when the one just granted expires. So a document that grants a
//!   `ReservoirQuota` carries it, and a document that grants none assigns no
//!   cadence at all and carries no `Interval`: the reporting service keeps the
//!   one it came with, whose statistics are then "for the last 10 seconds unless
//!   the service has been assigned a longer reporting interval after a previous
//!   call"
//!   (`smithy_query(query="shape",shape="SamplingStatisticsDocument")`). The
//!   table below drives every path that produces a target document, granting
//!   and not granting.
//!
//! * `GetTraceSummaries$TracesProcessedCount` is "the total number of traces
//!   processed, including traces that did not match the specified filter
//!   expression": the traces the REQUESTED WINDOW holds. So it moves with the
//!   window rather than reporting the whole store, never counts a trace the
//!   window excludes, is independent of the filter expression, and is the same
//!   on every page of one query. The table varies the window, the filter and the
//!   number of traces in and out of the window.

use serde_json::{json, Value};

use service_com_amazonaws_xray_awsxray::state::{
    DEFAULT_SAMPLING_RULE_NAME, SAMPLING_TARGET_INTERVAL_SECONDS,
};

use crate::sampling_targets::{
    client_id, create_rule, get_targets, get_targets_with_boost, list, now, statistics, target_for,
};
use crate::xray_wire::{array, root_document, trace_id, Answer, Xray};

// ===========================================================================
// 1. An Interval is served with the share a document grants, and only then
// ===========================================================================

/// Which path through `GetSamplingTargets` computed the answer. Every variant
/// produces at least one `SamplingTargetDocument`, by a different route through
/// the handler.
#[derive(Debug, Clone, Copy)]
enum Path {
    /// A rule's FIRST report, before any allocation cycle has divided its
    /// reservoir.
    FirstReport,
    /// A report answered the share the preceding cycle computed.
    AfterACycle,
    /// The service-managed catch-all rule on an account that created none.
    ServiceManagedDefaultRule,
    /// Two services reporting for one rule, each answered its own share.
    TwoServicesSharingOneReservoir,
    /// A rule whose configuration asks for a boost, reported for with boost
    /// statistics — the path that creates a document for the boost alone.
    BoostOnly,
    /// A rule reported for after it was deleted, answered from the allocation
    /// the service still holds.
    DeletedRuleStillHeld,
    /// Several rules answered in one call, ordered by priority.
    ManyRulesAtOnce,
}

/// How many target documents the path must produce.
fn expected_documents(path: Path) -> usize {
    match path {
        // One document per RULE, whichever service reported for it and however
        // many reports it carried, so every other path answers exactly one.
        Path::ManyRulesAtOnce => 3,
        _ => 1,
    }
}

/// Drive one path and return what `GetSamplingTargets` answered.
async fn targets_for(path: Path) -> Answer {
    let xray = Xray::new();
    let at = now();
    let client = client_id(7);
    match path {
        Path::FirstReport => {
            let created = create_rule(&xray, "first-report", 100, 0.05, 2, None).await;
            assert_eq!(
                created.status, 200,
                "case {path:?}: CreateSamplingRule must succeed, got {created:?}"
            );
            get_targets(&xray, &[statistics("first-report", &client, at, 20, 2, 0)]).await
        }
        Path::AfterACycle => {
            let created = create_rule(&xray, "after-a-cycle", 100, 0.5, 4, None).await;
            assert_eq!(
                created.status, 200,
                "case {path:?}: CreateSamplingRule must succeed, got {created:?}"
            );
            let documents = [statistics("after-a-cycle", &client, at, 40, 4, 0)];
            let _first = get_targets(&xray, &documents).await;
            get_targets(&xray, &documents).await
        }
        Path::ServiceManagedDefaultRule => {
            get_targets(
                &xray,
                &[statistics(DEFAULT_SAMPLING_RULE_NAME, &client, at, 5, 1, 0)],
            )
            .await
        }
        Path::TwoServicesSharingOneReservoir => {
            let created = create_rule(&xray, "shared", 1, 0.1, 10, None).await;
            assert_eq!(
                created.status, 200,
                "case {path:?}: CreateSamplingRule must succeed, got {created:?}"
            );
            let documents = [
                statistics("shared", &client, at, 60, 6, 0),
                statistics("shared", &client_id(8), at, 20, 2, 1),
            ];
            let _first = get_targets(&xray, &documents).await;
            get_targets(&xray, &documents).await
        }
        Path::BoostOnly => {
            let created = create_rule(&xray, "boosted", 100, 0.1, 2, Some((0.5, 5))).await;
            assert_eq!(
                created.status, 200,
                "case {path:?}: CreateSamplingRule must succeed, got {created:?}"
            );
            get_targets_with_boost(
                &xray,
                &[],
                &[json!({
                    "RuleName": "boosted",
                    "ServiceName": "checkout",
                    "Timestamp": at,
                    "AnomalyCount": 30,
                    "TotalCount": 100,
                    "SampledAnomalyCount": 1,
                })],
            )
            .await
        }
        Path::DeletedRuleStillHeld => {
            let created = create_rule(&xray, "departed", 100, 0.25, 3, None).await;
            assert_eq!(
                created.status, 200,
                "case {path:?}: CreateSamplingRule must succeed, got {created:?}"
            );
            let documents = [statistics("departed", &client, at, 30, 3, 0)];
            let _first = get_targets(&xray, &documents).await;
            let _second = get_targets(&xray, &documents).await;
            let removed = xray
                .call("/DeleteSamplingRule", &json!({"RuleName": "departed"}))
                .await;
            assert_eq!(
                removed.status, 200,
                "case {path:?}: DeleteSamplingRule must succeed, got {removed:?}"
            );
            get_targets(&xray, &documents).await
        }
        Path::ManyRulesAtOnce => {
            let rules = [("fast", 1, 1.0, 5), ("middling", 500, 0.5, 0), ("slow", 9999, 0.0, 1)];
            for (name, priority, rate, reservoir) in rules {
                let created = create_rule(&xray, name, priority, rate, reservoir, None).await;
                assert_eq!(
                    created.status, 200,
                    "case {path:?}: CreateSamplingRule must succeed, got {created:?}"
                );
            }
            let documents: Vec<Value> = rules
                .iter()
                .map(|(name, _, _, _)| statistics(name, &client, at, 10, 1, 0))
                .collect();
            let _first = get_targets(&xray, &documents).await;
            get_targets(&xray, &documents).await
        }
    }
}

#[tokio::test]
async fn an_interval_is_served_with_the_share_a_target_document_grants_and_only_then() {
    let paths = [
        Path::FirstReport,
        Path::AfterACycle,
        Path::ServiceManagedDefaultRule,
        Path::TwoServicesSharingOneReservoir,
        Path::BoostOnly,
        Path::DeletedRuleStillHeld,
        Path::ManyRulesAtOnce,
    ];

    // Every path must produce at least one document of each kind between them,
    // or the rule is only half exercised.
    let mut granting = 0usize;
    let mut granting_none = 0usize;

    for path in paths {
        let asked_at = now();
        let answer = targets_for(path).await;
        assert_eq!(
            answer.status, 200,
            "case {path:?}: GetSamplingTargets must succeed, got {answer:?}"
        );
        let documents = list(&answer, "SamplingTargetDocuments");
        assert_eq!(
            documents.len(),
            expected_documents(path),
            "case {path:?}: expected {} target document(s), got {documents:?}",
            expected_documents(path)
        );
        for document in &documents {
            let quota = document.get("ReservoirQuota").and_then(Value::as_i64);
            let interval = document.get("Interval").and_then(Value::as_i64);
            match quota {
                // A granted share comes with the cadence to come back for the
                // next one at, which is the span the granted share stays live
                // for: the service is told to ask again no later than when what
                // it holds expires.
                Some(_) => {
                    granting += 1;
                    assert_eq!(
                        interval,
                        Some(i64::from(SAMPLING_TARGET_INTERVAL_SECONDS)),
                        "case {path:?}: a document granting a share of {quota:?} must tell the service to come back in {SAMPLING_TARGET_INTERVAL_SECONDS}s, got {document:?}"
                    );
                    let ttl = document.get("ReservoirQuotaTTL").and_then(Value::as_f64);
                    let seconds = f64::from(SAMPLING_TARGET_INTERVAL_SECONDS);
                    assert!(
                        ttl.is_some_and(|ttl| ttl > asked_at && ttl - asked_at <= seconds + 1.0),
                        "case {path:?}: the interval of {interval:?}s must not outlast the share it is granted with (expiring at {ttl:?}, asked at {asked_at}), got {document:?}"
                    );
                }
                // Nothing was granted, so no cadence was assigned with it and
                // the reporting service keeps its own.
                None => {
                    granting_none += 1;
                    assert!(
                        document.get("Interval").is_none(),
                        "case {path:?}: a document granting no share assigns no interval, so Interval must not be serialized at all, got {document:?}"
                    );
                }
            }
        }
    }

    assert!(
        granting > 0 && granting_none > 0,
        "both sides of the rule must be exercised: {granting} granting document(s), {granting_none} granting none"
    );
}

#[tokio::test]
async fn a_target_document_keeps_every_other_member_it_computes() {
    // The member's presence tracks the share, so the same answer is checked to
    // still carry what it computes: the rule's stored rate and the share of the
    // reservoir the preceding cycle granted.
    let answer = targets_for(Path::AfterACycle).await;
    let target = target_for(&answer, "after-a-cycle")
        .unwrap_or_else(|| panic!("the reported rule must be answered a target, got {answer:?}"));
    assert_eq!(
        (
            target.get("FixedRate").and_then(Value::as_f64),
            target.get("ReservoirQuota").and_then(Value::as_i64),
        ),
        (Some(0.5), Some(4)),
        "the stored rate and the whole pool are still served, got {target:?}"
    );
    assert!(
        target
            .get("ReservoirQuotaTTL")
            .and_then(Value::as_f64)
            .is_some_and(|ttl| ttl > 0.0),
        "an allocated quota still expires at an instant, got {target:?}"
    );
}

// ===========================================================================
// 2. TracesProcessedCount is the count of the window, not of the store
// ===========================================================================

/// One seeded trace: its id, the `start_time` every segment of it reports, and
/// the service that reported it (what the `service()` filter selects on).
struct Seed {
    trace: String,
    start: f64,
    service: &'static str,
}

/// Seven traces at known, well-separated start times across three services.
fn seeds() -> Vec<Seed> {
    [
        (101_u32, 1_000.0_f64, "alpha"),
        (102, 1_100.0, "alpha"),
        (103, 1_200.0, "beta"),
        (104, 1_300.0, "beta"),
        (105, 1_400.0, "gamma"),
        (106, 1_500.0, "gamma"),
        (107, 1_600.0, "gamma"),
    ]
    .into_iter()
    .map(|(n, start, service)| Seed {
        trace: trace_id(n),
        start,
        service,
    })
    .collect()
}

/// One query: the window, an optional filter expression and the services that
/// expression selects — what the answer must report is DERIVED from the seeds,
/// not written down.
struct WindowCase {
    case: &'static str,
    start: f64,
    end: f64,
    /// The expression sent, and the service names it selects. `None` sends no
    /// expression at all, which selects every trace in the window.
    filter: Option<(&'static str, &'static [&'static str])>,
}

/// Rows differ in where the window falls relative to the seeded start times
/// (each boundary, one past the last, one before the first), how many traces it
/// holds (none, one, several, every one — more than one page), and whether a
/// filter expression selects all, some or none of them.
fn window_cases() -> Vec<WindowCase> {
    vec![
        WindowCase {
            case: "a window holding every stored trace",
            start: 900.0,
            end: 2_000.0,
            filter: None,
        },
        WindowCase {
            case: "a window past every stored trace",
            start: 1_700.0,
            end: 1_800.0,
            filter: None,
        },
        WindowCase {
            case: "a window before every stored trace",
            start: 0.0,
            end: 900.0,
            filter: None,
        },
        WindowCase {
            case: "an empty window at the first start time",
            start: 1_000.0,
            end: 1_000.0,
            filter: None,
        },
        WindowCase {
            case: "a window one millisecond wide at the first start time",
            start: 1_000.0,
            end: 1_000.001,
            filter: None,
        },
        WindowCase {
            case: "a window holding three of the seven",
            start: 1_100.0,
            end: 1_301.0,
            filter: None,
        },
        WindowCase {
            case: "a window at the last start time",
            start: 1_600.0,
            end: 1_700.0,
            filter: None,
        },
        WindowCase {
            case: "a filter selecting some of a window of three",
            start: 1_100.0,
            end: 1_301.0,
            filter: Some(("service(\"beta\")", &["beta"])),
        },
        WindowCase {
            case: "a filter selecting none of a window of three",
            start: 1_100.0,
            end: 1_301.0,
            filter: Some(("service(\"nobody\")", &[])),
        },
        WindowCase {
            case: "a filter selecting some of a window of everything",
            start: 900.0,
            end: 2_000.0,
            filter: Some(("service(\"gamma\")", &["gamma"])),
        },
        WindowCase {
            case: "a filter selecting every service of a window of everything",
            start: 900.0,
            end: 2_000.0,
            filter: Some((
                "service(\"alpha\") OR service(\"beta\") OR service(\"gamma\")",
                &["alpha", "beta", "gamma"],
            )),
        },
        WindowCase {
            case: "a filter over a window holding nothing",
            start: 1_700.0,
            end: 1_800.0,
            filter: Some(("service(\"alpha\")", &["alpha"])),
        },
    ]
}

#[tokio::test]
async fn traces_processed_count_is_the_traces_the_requested_window_holds() {
    let xray = Xray::new();
    let seeded = seeds();
    for (index, seed) in seeded.iter().enumerate() {
        let document = root_document(
            &format!("{:016x}", index + 1),
            &seed.trace,
            seed.service,
            seed.start,
            Some(seed.start + 0.5),
        )
        .to_string();
        let put = xray.put_segments(&[document]).await;
        assert_eq!(
            put.status, 200,
            "seeding {} must succeed, got {put:?}",
            seed.trace
        );
        assert!(
            array(&put.body, "UnprocessedTraceSegments").is_empty(),
            "seeding {} must process every document, got {put:?}",
            seed.trace
        );
    }

    for case in window_cases() {
        let label = case.case;
        // What the window holds, and which of those the filter selects: both
        // derived from the seeds, so the answer is compared against the rule
        // rather than against a remembered response.
        let in_window: Vec<&Seed> = seeded
            .iter()
            .filter(|seed| seed.start >= case.start && seed.start < case.end)
            .collect();
        let expected_processed = i64::try_from(in_window.len()).expect("the seed count fits");
        let mut expected_selected: Vec<&str> = in_window
            .iter()
            .filter(|seed| match case.filter {
                None => true,
                Some((_, services)) => services.contains(&seed.service),
            })
            .map(|seed| seed.trace.as_str())
            .collect();
        expected_selected.sort_unstable();

        // Every page of the one query, because the count describes the window
        // and so must not move as the pages do.
        let mut selected: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let got = xray
                .query_summaries(
                    case.start,
                    case.end,
                    case.filter.map(|(filter, _)| filter),
                    token.as_deref(),
                )
                .await;
            assert_eq!(
                got.status, 200,
                "case {label:?}: GetTraceSummaries must succeed, got {got:?}"
            );
            pages += 1;
            let processed = got.get("TracesProcessedCount").as_i64();
            assert_eq!(
                processed,
                Some(expected_processed),
                "case {label:?}: page {pages} of window [{}, {}) holds {expected_processed} trace(s), got {processed:?}",
                case.start,
                case.end
            );
            selected.extend(
                array(&got.body, "TraceSummaries")
                    .iter()
                    .filter_map(|summary| summary.get("Id").and_then(Value::as_str))
                    .map(str::to_string),
            );
            match got.get("NextToken").as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }

        selected.sort_unstable();
        assert_eq!(
            selected, expected_selected,
            "case {label:?}: the window and the filter must select {expected_selected:?}, got {selected:?}"
        );
        // The count is what the window held, so it can never be smaller than
        // what the same query served out of it.
        let served = i64::try_from(selected.len()).expect("the seed count fits");
        assert!(
            served <= expected_processed,
            "case {label:?}: {served} summaries were served out of {expected_processed} processed"
        );
    }
}

#[tokio::test]
async fn a_window_holding_nothing_reports_none_processed_however_much_is_stored() {
    // The class the recorded divergence belongs to: a store that holds traces,
    // queried over a window that holds none of them. The count is of the window,
    // so it is zero — and it stays zero as the store grows.
    let xray = Xray::new();
    for (index, seed) in seeds().iter().enumerate() {
        let document = root_document(
            &format!("{:016x}", index + 1),
            &seed.trace,
            seed.service,
            seed.start,
            Some(seed.start + 0.5),
        )
        .to_string();
        let put = xray.put_segments(&[document]).await;
        assert_eq!(
            put.status, 200,
            "seeding {} must succeed, got {put:?}",
            seed.trace
        );

        let got = xray.summaries(5_000.0, 6_000.0, None).await;
        assert_eq!(
            got.status, 200,
            "after {} trace(s): GetTraceSummaries must succeed, got {got:?}",
            index + 1
        );
        assert!(
            array(&got.body, "TraceSummaries").is_empty(),
            "after {} trace(s): the window holds nothing, got {got:?}",
            index + 1
        );
        let processed = got.get("TracesProcessedCount").as_i64();
        assert_eq!(
            processed,
            Some(0),
            "after {} trace(s): a window holding nothing processed none, got {processed:?}",
            index + 1
        );
    }
}
