// SPDX-License-Identifier: CC0-1.0

//! `GetInsightSummaries`: the stored insights a request selects, end to end
//! over the wire.
//!
//! Every case drives the same front door the production router does (the
//! harness in `trace_ingest_and_groups`), so the assertions are about HTTP
//! responses rather than internal state. Nothing in the X-Ray API creates an
//! insight — X-Ray mints one itself from anomaly detection — so the cases that
//! need one seed the insight store the way the `GetInsight` cases do, and then
//! read it back only through the wire.
//!
//! The tests are tables, and each row differs along what the rule it checks
//! generalises over — the position of the window against an insight's own
//! interval, which bounds the insight reports, its group, its state, the number
//! of insights relative to a page — because the rules are:
//!
//! * the listing is of the STORED insights, every member of each
//!   `InsightSummary` being the stored insight's, so an id this operation lists
//!   is an id `GetInsight` resolves and an empty store lists nothing;
//! * an insight is selected by the OVERLAP of its own interval with the
//!   requested window, not by containment, and a bound the insight does not
//!   report bounds nothing;
//! * a group selector narrows the listing to the insights of that group, by
//!   either `GroupName` or `GroupARN`, and `States` narrows it to one
//!   `InsightState`;
//! * the operation is `@paginated`: `MaxResults` bounds a page, a `NextToken`
//!   appears only when the listing was truncated, and resuming from it yields
//!   the remainder exactly once;
//! * a request the operation cannot satisfy — a window naming no interval of
//!   recorded traffic, a selector naming no group or naming two, a `States`
//!   value outside `InsightState`, a token this service never issued — is
//!   REFUSED with `InvalidRequestException` at HTTP 400, never answered with a
//!   200 that ignored it.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use service_com_amazonaws_xray_awsxray::ops::types::{
    AnomalousService, Insight, RequestImpactStatistics, ServiceId,
};
use service_com_amazonaws_xray_awsxray::state::ComAmazonawsXrayAwsxrayState;

use crate::trace_ingest_and_groups::{array, close, Answer, Xray};

/// Now, in the epoch seconds every timestamp on this wire travels as — the
/// crate's one clock, the same one the handlers read.
fn now() -> f64 {
    service_com_amazonaws_xray_awsxray::state::now_epoch_seconds()
}

/// One insight to seed: its id, group, state and the interval it reports.
#[derive(Debug, Clone)]
struct Seed {
    /// A UUID the modelled `@pattern` on `InsightId` accepts.
    id: &'static str,
    group_name: &'static str,
    /// The group ARN the insight carries, once the group has been created.
    group_arn: Option<String>,
    state: &'static str,
    /// `StartTime`, absent when the insight reports no start.
    start: Option<f64>,
    /// `EndTime`, absent while the insight has not ended.
    end: Option<f64>,
    /// The root cause service's name.
    service: &'static str,
    /// `RootCauseServiceRequestImpactStatistics` as (fault, ok, total).
    faults: (i64, i64, i64),
}

impl Seed {
    /// The stored `Insight` this row seeds. Every member the handler reports is
    /// set here, so a difference in the answer is a difference the handler made.
    fn insight(&self) -> Insight {
        let service = ServiceId {
            name: Some(self.service.to_string()),
            names: Some(vec![self.service.to_string()]),
            account_id: None,
            r#type: Some("AWS::EC2::Instance".to_string()),
        };
        let statistics = RequestImpactStatistics {
            fault_count: Some(self.faults.0),
            ok_count: Some(self.faults.1),
            total_count: Some(self.faults.2),
        };
        Insight {
            insight_id: Some(self.id.to_string()),
            group_arn: self.group_arn.clone(),
            group_name: Some(self.group_name.to_string()),
            root_cause_service_id: Some(service.clone()),
            // `InsightCategory` publishes exactly one value.
            categories: Some(vec!["FAULT".to_string()]),
            state: Some(self.state.to_string()),
            start_time: self.start.map(::aws_smithy_types::DateTime::from_secs_f64),
            end_time: self.end.map(::aws_smithy_types::DateTime::from_secs_f64),
            summary: Some(format!("{} of {} requests faulted", self.faults.0, self.faults.2)),
            client_request_impact_statistics: Some(statistics.clone()),
            root_cause_service_request_impact_statistics: Some(statistics),
            top_anomalous_services: Some(vec![AnomalousService {
                service_id: Some(service),
            }]),
        }
    }
}

/// Seed every row into the insight store the handler reads.
fn seed(engine: &std::sync::Arc<engine::InMemoryEngine>, seeds: &[Seed]) {
    let state = engine.state::<ComAmazonawsXrayAwsxrayState>();
    let mut store = state.lock();
    for seed in seeds {
        store
            .insights
            .insert(seed.id.to_string(), seed.insight());
    }
}

/// Create one group, returning the ARN the service minted for it. A group
/// carries a filter expression because `CreateGroup` requires one.
async fn create_group(xray: &Xray, label: &str, name: &str) -> String {
    let created = xray
        .call(
            "/CreateGroup",
            &json!({"GroupName": name, "FilterExpression": "fault = true"}),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case {label:?}: creating group {name:?} must succeed, got {created:?}"
    );
    created
        .get("Group")
        .get("GroupARN")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// `GetInsightSummaries` over `[start, end]`, plus any extra members.
async fn summaries(xray: &Xray, start: f64, end: f64, extra: &Value) -> Answer {
    let mut body = json!({"StartTime": start, "EndTime": end});
    for (member, value) in extra.as_object().into_iter().flatten() {
        body[member] = value.clone();
    }
    xray.call("/InsightSummaries", &body).await
}

/// The `InsightId`s of one answer, in the order it listed them.
fn ids(answer: &Answer) -> Vec<String> {
    array(&answer.body, "InsightSummaries")
        .iter()
        .filter_map(|insight| insight.get("InsightId").and_then(Value::as_str))
        .map(str::to_string)
        .collect()
}

/// The three `RequestImpactStatistics` members of one insight member, as
/// (`FaultCount`, `OkCount`, `TotalCount`); `-1` stands for anything absent.
fn impact(insight: &Value, member: &str) -> (i64, i64, i64) {
    let statistics = insight.get(member);
    let count = |name: &str| {
        statistics
            .and_then(|value| value.get(name))
            .and_then(Value::as_i64)
            .unwrap_or(-1)
    };
    (count("FaultCount"), count("OkCount"), count("TotalCount"))
}

// =============================================================================
// 1. An empty store lists nothing, and every listed member is the stored one
// =============================================================================

#[tokio::test]
async fn an_empty_insight_store_lists_nothing_over_any_window() {
    let xray = Xray::new();
    // Rows differ in the width and the age of the window; none of them can
    // invent an insight, because nothing in the API creates one.
    for (label, start, end) in [
        ("the last hour", now() - 3_600.0, now()),
        ("a one-second window", now() - 1.0, now()),
        ("a window a day wide", now() - 86_400.0, now()),
        ("a window well in the past", now() - 1_000_000.0, now() - 999_000.0),
    ] {
        let answered = summaries(&xray, start, end, &json!({})).await;
        assert_eq!(
            answered.status, 200,
            "case {label:?}: a well-formed window must succeed, got {answered:?}"
        );
        assert_eq!(
            array(&answered.body, "InsightSummaries"),
            &[] as &[Value],
            "case {label:?}: a fresh account holds no insight, got {answered:?}"
        );
        assert!(
            answered.get("NextToken").is_null(),
            "case {label:?}: an empty listing carries no NextToken, got {answered:?}"
        );
    }
}

#[tokio::test]
async fn every_listed_member_is_the_stored_insights_and_resolves_through_get_insight() {
    let (xray, engine) = Xray::with_engine();
    let arn = create_group(&xray, "members", "watched").await;

    // Rows differ in state, in which bounds the insight reports, and in its
    // counts — so no single shape of insight can satisfy them all.
    let seeds = vec![
        Seed {
            id: "11111111-2222-4333-8444-555555555555",
            group_name: "watched",
            group_arn: Some(arn.clone()),
            state: "ACTIVE",
            start: Some(now() - 600.0),
            end: None,
            service: "api",
            faults: (7, 93, 100),
        },
        Seed {
            id: "22222222-3333-4444-9555-666666666666",
            group_name: "watched",
            group_arn: Some(arn.clone()),
            state: "CLOSED",
            start: Some(now() - 1_200.0),
            end: Some(now() - 900.0),
            service: "payments",
            faults: (1, 0, 1),
        },
    ];
    seed(&engine, &seeds);

    let answered = summaries(&xray, now() - 3_600.0, now(), &json!({})).await;
    assert_eq!(
        answered.status, 200,
        "the listing must succeed, got {answered:?}"
    );
    let listed = array(&answered.body, "InsightSummaries");
    assert_eq!(
        listed.len(),
        seeds.len(),
        "every stored insight in the window must be listed, got {answered:?}"
    );

    for (seed, insight) in seeds.iter().zip(listed) {
        let label = seed.id;
        assert_eq!(
            insight.get("InsightId").and_then(Value::as_str),
            Some(seed.id),
            "case {label:?}: the listing is in InsightId order, got {answered:?}"
        );
        assert_eq!(
            insight.get("GroupName").and_then(Value::as_str),
            Some(seed.group_name),
            "case {label:?}: expected the stored group name, got {insight:?}"
        );
        assert_eq!(
            insight.get("GroupARN").and_then(Value::as_str),
            Some(arn.as_str()),
            "case {label:?}: expected the stored group ARN, got {insight:?}"
        );
        assert_eq!(
            insight.get("State").and_then(Value::as_str),
            Some(seed.state),
            "case {label:?}: expected the stored state, got {insight:?}"
        );
        assert_eq!(
            insight
                .get("RootCauseServiceId")
                .and_then(|service| service.get("Name"))
                .and_then(Value::as_str),
            Some(seed.service),
            "case {label:?}: expected the stored root cause service, got {insight:?}"
        );
        assert_eq!(
            insight
                .get("Categories")
                .and_then(Value::as_array)
                .map(Vec::as_slice),
            Some([json!("FAULT")].as_slice()),
            "case {label:?}: expected the stored category, got {insight:?}"
        );
        let start = insight.get("StartTime").and_then(Value::as_f64);
        assert!(
            start.is_some_and(|start| seed.start.is_some_and(|stored| close(start, stored))),
            "case {label:?}: expected StartTime {:?}, got {start:?}",
            seed.start
        );
        let end = insight.get("EndTime").and_then(Value::as_f64);
        match seed.end {
            Some(stored) => assert!(
                end.is_some_and(|end| close(end, stored)),
                "case {label:?}: expected EndTime {stored}, got {end:?}"
            ),
            None => assert!(
                end.is_none(),
                "case {label:?}: an insight that has not ended reports no EndTime, got {end:?}"
            ),
        }
        assert_eq!(
            impact(insight, "RootCauseServiceRequestImpactStatistics"),
            seed.faults,
            "case {label:?}: expected the stored root cause statistics, got {insight:?}"
        );
        assert_eq!(
            impact(insight, "ClientRequestImpactStatistics"),
            seed.faults,
            "case {label:?}: expected the stored client statistics, got {insight:?}"
        );
        assert_eq!(
            array(insight, "TopAnomalousServices")
                .first()
                .and_then(|anomalous| anomalous.get("ServiceId"))
                .and_then(|service| service.get("Name"))
                .and_then(Value::as_str),
            Some(seed.service),
            "case {label:?}: expected the stored top anomalous service, got {insight:?}"
        );
        assert_eq!(
            insight.get("Summary").and_then(Value::as_str),
            Some(format!("{} of {} requests faulted", seed.faults.0, seed.faults.2).as_str()),
            "case {label:?}: expected the stored summary, got {insight:?}"
        );

        // The two readers of the insight store agree: an id this operation
        // lists is an id `GetInsight` resolves.
        let read = xray.call("/Insight", &json!({"InsightId": seed.id})).await;
        assert_eq!(
            read.status, 200,
            "case {label:?}: a listed InsightId must resolve through GetInsight, got {read:?}"
        );
        assert_eq!(
            read.get("Insight").get("State").and_then(Value::as_str),
            Some(seed.state),
            "case {label:?}: both readers must answer the same stored insight, got {read:?}"
        );
    }

    // A listing does not consume or move anything: the same request answers the
    // same thing again.
    let again = summaries(&xray, now() - 3_600.0, now(), &json!({})).await;
    assert_eq!(
        ids(&answered),
        ids(&again),
        "a read must not change what a later read sees: got {answered:?} then {again:?}"
    );
}

// =============================================================================
// 2. The window selects by overlap with the insight's own interval
// =============================================================================

#[tokio::test]
async fn the_window_selects_an_insight_whose_interval_overlaps_it() {
    // One insight per shape of reported interval, so each row below is tested
    // against a closed interval, an open-ended one, and one with no start.
    let base = now() - 10_000.0;
    let closed = "33333333-4444-4555-8666-777777777777";
    let open = "44444444-5555-4666-9777-888888888888";
    let unbounded_start = "55555555-6666-4777-a888-999999999999";

    // (label, window start, window end, the ids that must be listed)
    let rows: Vec<(&str, f64, f64, Vec<&str>)> = vec![
        (
            "window entirely before every interval",
            base - 2_000.0,
            base - 1_000.0,
            // Only the insight that reports no start can have been in progress
            // then: a bound it does not report bounds nothing.
            vec![unbounded_start],
        ),
        (
            "window ending exactly where the intervals begin",
            base - 1_000.0,
            base,
            vec![unbounded_start],
        ),
        (
            "window ending just after they begin",
            base - 1_000.0,
            base + 1.0,
            vec![closed, open, unbounded_start],
        ),
        (
            "window inside the closed interval",
            base + 10.0,
            base + 20.0,
            vec![closed, open, unbounded_start],
        ),
        (
            "window beginning exactly at the closed interval's end",
            base + 100.0,
            base + 200.0,
            vec![closed, open, unbounded_start],
        ),
        (
            "window beginning after the closed interval ended",
            base + 101.0,
            base + 200.0,
            // The closed one had ended; the other two had not.
            vec![open, unbounded_start],
        ),
        (
            "window containing every interval",
            base - 1_000.0,
            base + 1_000.0,
            vec![closed, open, unbounded_start],
        ),
        (
            "zero-width window at the intervals' start",
            base,
            base,
            vec![unbounded_start],
        ),
    ];

    let (xray, engine) = Xray::with_engine();
    seed(
        &engine,
        &[
            Seed {
                id: closed,
                group_name: "watched",
                group_arn: None,
                state: "CLOSED",
                start: Some(base),
                end: Some(base + 100.0),
                service: "api",
                faults: (1, 1, 2),
            },
            Seed {
                id: open,
                group_name: "watched",
                group_arn: None,
                state: "ACTIVE",
                start: Some(base),
                end: None,
                service: "payments",
                faults: (2, 0, 2),
            },
            Seed {
                id: unbounded_start,
                group_name: "watched",
                group_arn: None,
                state: "ACTIVE",
                start: None,
                end: None,
                service: "search",
                faults: (3, 0, 3),
            },
        ],
    );

    for (label, start, end, expected) in rows {
        let answered = summaries(&xray, start, end, &json!({})).await;
        assert_eq!(
            answered.status, 200,
            "case {label:?}: a well-formed window must succeed, got {answered:?}"
        );
        assert_eq!(
            ids(&answered),
            expected
                .iter()
                .map(|id| (*id).to_string())
                .collect::<Vec<String>>(),
            "case {label:?}: expected {expected:?}, got {answered:?}"
        );
    }
}

// =============================================================================
// 3. A group selector and States narrow the listing
// =============================================================================

#[tokio::test]
async fn a_group_selector_and_states_narrow_the_listing() {
    let (xray, engine) = Xray::with_engine();
    let alpha_arn = create_group(&xray, "selectors", "alpha").await;
    let beta_arn = create_group(&xray, "selectors", "beta").await;

    let alpha_active = "66666666-7777-4888-8999-aaaaaaaaaaaa";
    let alpha_closed = "77777777-8888-4999-9aaa-bbbbbbbbbbbb";
    let beta_active = "88888888-9999-4aaa-abbb-cccccccccccc";
    seed(
        &engine,
        &[
            Seed {
                id: alpha_active,
                group_name: "alpha",
                group_arn: Some(alpha_arn.clone()),
                state: "ACTIVE",
                start: Some(now() - 300.0),
                end: None,
                service: "api",
                faults: (1, 0, 1),
            },
            Seed {
                id: alpha_closed,
                group_name: "alpha",
                group_arn: Some(alpha_arn.clone()),
                state: "CLOSED",
                start: Some(now() - 600.0),
                end: Some(now() - 500.0),
                service: "api",
                faults: (1, 4, 5),
            },
            Seed {
                id: beta_active,
                group_name: "beta",
                group_arn: Some(beta_arn.clone()),
                state: "ACTIVE",
                start: Some(now() - 200.0),
                end: None,
                service: "search",
                faults: (2, 0, 2),
            },
        ],
    );

    // Rows differ in which selector names the group and which state is asked
    // for, including both together and neither.
    let rows: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "no selector and no States",
            json!({}),
            vec![alpha_active, alpha_closed, beta_active],
        ),
        (
            "GroupName",
            json!({"GroupName": "alpha"}),
            vec![alpha_active, alpha_closed],
        ),
        (
            "the other group by GroupName",
            json!({"GroupName": "beta"}),
            vec![beta_active],
        ),
        (
            "GroupARN",
            json!({"GroupARN": alpha_arn.clone()}),
            vec![alpha_active, alpha_closed],
        ),
        (
            "the reserved Default group, which holds no insight",
            json!({"GroupName": "Default"}),
            vec![],
        ),
        (
            "States ACTIVE",
            json!({"States": ["ACTIVE"]}),
            vec![alpha_active, beta_active],
        ),
        (
            "States CLOSED",
            json!({"States": ["CLOSED"]}),
            vec![alpha_closed],
        ),
        ("an empty States list", json!({"States": []}), vec![alpha_active, alpha_closed, beta_active]),
        (
            "a group and a state together",
            json!({"GroupName": "alpha", "States": ["CLOSED"]}),
            vec![alpha_closed],
        ),
        (
            "a group and a state that select nothing together",
            json!({"GroupName": "beta", "States": ["CLOSED"]}),
            vec![],
        ),
    ];

    for (label, extra, expected) in rows {
        let answered = summaries(&xray, now() - 3_600.0, now(), &extra).await;
        assert_eq!(
            answered.status, 200,
            "case {label:?}: the request must succeed, got {answered:?}"
        );
        assert_eq!(
            ids(&answered),
            expected
                .iter()
                .map(|id| (*id).to_string())
                .collect::<Vec<String>>(),
            "case {label:?}: expected {expected:?}, got {answered:?}"
        );
    }
}

// =============================================================================
// 4. The @paginated contract
// =============================================================================

#[tokio::test]
async fn a_next_token_appears_only_when_truncated_and_resumes_exactly_once() {
    use service_com_amazonaws_xray_awsxray::ops::get_insight_summaries::INSIGHT_SUMMARIES_PAGE_SIZE;

    // Ids are padded hex, so sorting them orders them by index — which is the
    // order the pages must walk.
    let ids_for = |count: usize| -> Vec<String> {
        (0..count)
            .map(|index| format!("{index:08x}-0000-4000-8000-{index:012x}"))
            .collect()
    };

    // Counts either side of a page, and page sizes either side of the counts —
    // including the model's own maximum of 100.
    for count in [0_usize, 1, 3, INSIGHT_SUMMARIES_PAGE_SIZE + 2] {
        for max_results in [None, Some(1), Some(2), Some(100)] {
            let label = format!("{count} insight(s), MaxResults {max_results:?}");
            let (xray, engine) = Xray::with_engine();
            let all = ids_for(count);
            {
                let state = engine.state::<ComAmazonawsXrayAwsxrayState>();
                let mut store = state.lock();
                for (index, id) in all.iter().enumerate() {
                    #[allow(clippy::cast_precision_loss)]
                    let started = now() - 1_000.0 + index as f64;
                    store.insights.insert(
                        id.clone(),
                        Insight {
                            insight_id: Some(id.clone()),
                            group_name: Some("watched".to_string()),
                            state: Some("ACTIVE".to_string()),
                            start_time: Some(::aws_smithy_types::DateTime::from_secs_f64(started)),
                            ..Insight::default()
                        },
                    );
                }
            }

            let page_size = max_results.unwrap_or(INSIGHT_SUMMARIES_PAGE_SIZE);
            let mut seen: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            let mut pages = 0;
            loop {
                let mut extra = json!({});
                if let Some(max) = max_results {
                    extra["MaxResults"] = json!(max);
                }
                if let Some(token) = &token {
                    extra["NextToken"] = json!(token);
                }
                let answered = summaries(&xray, now() - 3_600.0, now(), &extra).await;
                assert_eq!(
                    answered.status, 200,
                    "case {label:?}: page {pages} must succeed, got {answered:?}"
                );
                let page = ids(&answered);
                assert!(
                    page.len() <= page_size,
                    "case {label:?}: a page carries at most {page_size}, got {}",
                    page.len()
                );
                seen.extend(page.clone());
                pages += 1;
                assert!(
                    pages <= count + 2,
                    "case {label:?}: pagination must terminate, got {pages} pages"
                );
                match answered.get("NextToken").as_str().map(str::to_string) {
                    Some(next) => {
                        assert_eq!(
                            page.len(),
                            page_size,
                            "case {label:?}: a NextToken is due only on a full page, got {answered:?}"
                        );
                        assert!(
                            all.contains(&next),
                            "case {label:?}: a NextToken must name the next insight, got {next:?}"
                        );
                        token = Some(next);
                    }
                    None => break,
                }
            }

            assert_eq!(
                seen, all,
                "case {label:?}: the pages must yield every insight once, in order"
            );
            let distinct: BTreeSet<&String> = seen.iter().collect();
            assert_eq!(
                distinct.len(),
                count,
                "case {label:?}: no insight may repeat across pages, got {seen:?}"
            );
        }
    }
}

// =============================================================================
// 5. A request the operation cannot satisfy is refused as invalid
// =============================================================================

#[tokio::test]
async fn a_request_the_operation_cannot_satisfy_is_refused_as_invalid() {
    use service_com_amazonaws_xray_awsxray::state::MAX_WINDOW_FUTURE_SKEW_SECONDS;

    let (xray, engine) = Xray::with_engine();
    let arn = create_group(&xray, "refusals", "watched").await;
    let stored = "99999999-aaaa-4bbb-8ccc-dddddddddddd";
    seed(
        &engine,
        &[Seed {
            id: stored,
            group_name: "watched",
            group_arn: Some(arn.clone()),
            state: "ACTIVE",
            start: Some(now() - 300.0),
            end: None,
            service: "api",
            faults: (1, 0, 1),
        }],
    );

    // Rows span the ways the request can be invalid, and each one names the code
    // it must come back as: the request layer refuses a MODELLED constraint
    // (an absent required member, a bound on `MaxResults`, the length of
    // `States`, a value outside `InsightState`) with `ValidationException`, and
    // the handler refuses what only it can judge — each refusable window, a
    // selector naming no group or naming two, a token this service never issued
    // — with the `InvalidRequestException` it models.
    let rows: Vec<(&str, &str, Value)> = vec![
        (
            "StartTime absent",
            "ValidationException",
            json!({"EndTime": now()}),
        ),
        (
            "EndTime absent",
            "ValidationException",
            json!({"StartTime": now() - 1.0}),
        ),
        (
            "MaxResults below the model's minimum",
            "ValidationException",
            json!({"StartTime": now() - 1.0, "EndTime": now(), "MaxResults": 0}),
        ),
        (
            "MaxResults above the model's maximum",
            "ValidationException",
            json!({"StartTime": now() - 1.0, "EndTime": now(), "MaxResults": 101}),
        ),
        (
            "more States than the model's maximum",
            "ValidationException",
            json!({
                "StartTime": now() - 1.0,
                "EndTime": now(),
                "States": ["ACTIVE", "CLOSED"],
            }),
        ),
        (
            "a window running backwards",
            "InvalidRequestException",
            json!({"StartTime": now(), "EndTime": now() - 3_600.0}),
        ),
        (
            "a window past the present",
            "InvalidRequestException",
            json!({
                "StartTime": now() + MAX_WINDOW_FUTURE_SKEW_SECONDS + 60.0,
                "EndTime": now() + MAX_WINDOW_FUTURE_SKEW_SECONDS + 120.0,
            }),
        ),
        (
            "a millisecond-scale epoch value read as seconds",
            "InvalidRequestException",
            json!({"StartTime": now() * 1_000.0, "EndTime": now() * 1_000.0 + 60.0}),
        ),
        (
            "GroupName naming no group",
            "InvalidRequestException",
            json!({"StartTime": now() - 1.0, "EndTime": now(), "GroupName": "absent"}),
        ),
        (
            "GroupARN naming no group",
            "InvalidRequestException",
            json!({
                "StartTime": now() - 1.0,
                "EndTime": now(),
                "GroupARN": "arn:aws:xray:us-east-1:000000000000:group/absent/000000000000",
            }),
        ),
        (
            "both selectors at once, naming one group",
            "InvalidRequestException",
            json!({
                "StartTime": now() - 1.0,
                "EndTime": now(),
                "GroupName": "watched",
                "GroupARN": arn.clone(),
            }),
        ),
        (
            "a States value outside InsightState",
            "ValidationException",
            json!({"StartTime": now() - 1.0, "EndTime": now(), "States": ["PENDING"]}),
        ),
        (
            "a NextToken this service never issued",
            "InvalidRequestException",
            json!({"StartTime": now() - 1.0, "EndTime": now(), "NextToken": "never-issued"}),
        ),
        (
            // Well formed as an id, and past every stored one.
            "a NextToken beyond the listed keyspace",
            "InvalidRequestException",
            json!({
                "StartTime": now() - 3_600.0,
                "EndTime": now(),
                "NextToken": "ffffffff-ffff-4fff-bfff-ffffffffffff",
            }),
        ),
    ];

    for (label, expected_code, body) in rows {
        let refused = xray.call("/InsightSummaries", &body).await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some(expected_code)),
            "case {label:?}: expected 400 {expected_code}, got {refused:?}"
        );
        assert!(
            refused.message().is_some_and(|message| !message.is_empty()),
            "case {label:?}: a refusal must say what was wrong, got {refused:?}"
        );
    }

    // And the stored insight is still listed: a refused request disturbed
    // nothing.
    let answered = summaries(&xray, now() - 3_600.0, now(), &json!({})).await;
    assert_eq!(
        ids(&answered),
        vec![stored.to_string()],
        "a refused request must not change what the store answers, got {answered:?}"
    );
}
