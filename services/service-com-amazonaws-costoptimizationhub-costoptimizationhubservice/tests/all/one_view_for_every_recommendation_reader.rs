// SPDX-License-Identifier: CC0-1.0

//! The ONE view every recommendation reader is served, driven end-to-end over
//! the wire (`awsJson1_0`) through the same multi-service router the emulator
//! boots.
//!
//! The rule under test: a reader of recommendations reports the rows the
//! accounts its read addresses may be read for — an enrolled account whose
//! analysis has come due — with their money members computed under the caller's
//! effective `savingsEstimationMode`. `ListRecommendations`,
//! `ListRecommendationSummaries` and `GetRecommendation` differ only in which
//! accounts they address and in how much of a row they render, so the mode they
//! report under and the rows they admit have to move together: these tests drive
//! the shared step through all three readers over a table of stored modes and a
//! table of enrollment states, so the rule cannot be changed for one reader and
//! left behind in another.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cost_optimization_hub::{
    call, enrolled_router_with_discounts_visible, error_code, ids, ok, router, ANALYSIS_WAIT,
};

/// The money members a list entry and the detail view of the same row share, so
/// a mode resolved differently by one of the two readers shows up here.
const MONEY_MEMBERS: [&str; 3] = [
    "estimatedMonthlyCost",
    "estimatedMonthlySavings",
    "estimatedSavingsPercentage",
];

/// Two money figures are the same figure: every reader rounds to cents, so
/// anything inside half a cent is the same value.
fn same_money(actual: f64, expected: f64) -> bool {
    (actual - expected).abs() < 0.005
}

/// One money member of an answer, as `NaN` when the answer carries none — which
/// no comparison treats as equal, so a missing member fails the case that reads
/// it.
fn money(payload: &Value, member: &str) -> f64 {
    payload[member].as_f64().unwrap_or(f64::NAN)
}

/// Every recommendation `ListRecommendations` serves this caller, as whole
/// entries. `include_all` chooses between the whole selection and the
/// one-per-resource de-duped view the summaries aggregate.
async fn listed(router: &MultiServiceRouter, include_all: bool) -> Vec<Value> {
    let payload = ok(
        router,
        "ListRecommendations",
        &json!({"includeAllRecommendations": include_all}),
    )
    .await;
    payload["items"].as_array().cloned().unwrap_or_default()
}

/// The `(recommendationId, estimatedMonthlySavings)` pairs of a selection, so
/// two stored modes can be compared row by row.
fn savings_by_id(items: &[Value]) -> Vec<(String, f64)> {
    items
        .iter()
        .map(|item| {
            (
                item["recommendationId"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                money(item, "estimatedMonthlySavings"),
            )
        })
        .collect()
}

/// Every reader reports the same row under the same resolved mode: the detail
/// view's money members are the list entry's, and the summaries total the
/// de-duped selection at those same figures, whichever `savingsEstimationMode`
/// the account has stored.
///
/// The table spans what the rule generalises over: each value of the model's
/// `SavingsEstimationMode` enum (`smithy_query(query="shape",
/// shape="SavingsEstimationMode")`: `AfterDiscounts`, `BeforeDiscounts`), the
/// account that has stored no preference at all and so reads under the mode the
/// service defaults to, and a row that moves the stored mode back — so no
/// reader can be answering from a mode it resolved once.
#[tokio::test]
async fn every_reader_reports_one_resolved_savings_estimation_mode() {
    // The discounted mode is available only where the organization has made its
    // member accounts' discounts visible, so the caller here is the
    // organization's management account with that preference set.
    let router = enrolled_router_with_discounts_visible().await;

    // (case, the mode stored before the reads; `None` stores nothing)
    let cases: [(&str, Option<&str>); 5] = [
        ("no stored preference", None),
        ("before discounts", Some("BeforeDiscounts")),
        ("after discounts", Some("AfterDiscounts")),
        ("back to before discounts", Some("BeforeDiscounts")),
        ("after discounts again", Some("AfterDiscounts")),
    ];

    let mut reported: Vec<(&str, Vec<(String, f64)>)> = Vec::new();
    for (case, mode) in cases {
        if let Some(mode) = mode {
            ok(
                &router,
                "UpdatePreferences",
                &json!({"savingsEstimationMode": mode}),
            )
            .await;
        }

        let items = listed(&router, true).await;
        assert!(
            !items.is_empty(),
            "case {case:?}: expected the enrolled account's rows to read, got {items:?}"
        );

        for item in &items {
            let recommendation_id = item["recommendationId"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let detail = ok(
                &router,
                "GetRecommendation",
                &json!({"recommendationId": recommendation_id}),
            )
            .await;
            for member in MONEY_MEMBERS {
                let expected = money(item, member);
                let actual = money(&detail, member);
                assert!(
                    same_money(actual, expected),
                    "case {case:?} id {recommendation_id:?}: expected {member} {expected} as the \
                     list reports it, got {actual} in {detail}"
                );
            }
        }

        // The third reader of the same view: its de-duped total is the saving
        // the one-per-resource selection adds up to, at the money that
        // selection carries under this stored mode.
        let deduped = listed(&router, false).await;
        let expected: f64 = deduped
            .iter()
            .map(|item| money(item, "estimatedMonthlySavings"))
            .sum();
        let summary = ok(
            &router,
            "ListRecommendationSummaries",
            &json!({"groupBy": "Region"}),
        )
        .await;
        let actual = money(&summary, "estimatedTotalDedupedSavings");
        assert!(
            same_money(actual, expected),
            "case {case:?}: expected the de-duped total {expected} the list adds up to, got \
             {actual} in {summary}"
        );

        reported.push((case, savings_by_id(&items)));
    }

    // The table is not vacuous: the two modes report different money for the
    // same rows, so the agreement above is agreement on a mode that matters.
    let before = &reported[1];
    let after = &reported[2];
    assert_eq!(
        before.1.len(),
        after.1.len(),
        "case {:?} vs {:?}: expected the same rows under both modes, got {:?} and {:?}",
        before.0,
        after.0,
        before.1,
        after.1
    );
    assert!(
        before
            .1
            .iter()
            .zip(after.1.iter())
            .any(|((_, before), (_, after))| !same_money(*after, *before)),
        "case {:?} vs {:?}: expected the stored mode to change the reported money, got {:?} for \
         both",
        before.0,
        after.0,
        before.1
    );
}

/// The rows every reader admits follow the same enrollment gate: while the
/// account is enrolled and its analysis has come due, the list serves its rows
/// and each of those ids reads back by id; while it is not, the list serves
/// none, the summaries total nothing, and every one of those ids is a 404 —
/// never a row one reader still admits.
///
/// The table spans the states the gate distinguishes, and repeats them, because
/// opting in again starts a fresh analysis: the rows have to leave and return
/// for all three readers at once.
#[tokio::test]
async fn the_rows_every_reader_admits_follow_one_enrollment_gate() {
    let router = router();

    // Before any enrollment the account has opted into nothing, so no reader
    // admits a row.
    assert!(
        listed(&router, true).await.is_empty(),
        "case before enrollment: expected no rows at all"
    );

    // (case, the status to move the account to, whether its rows are readable
    // afterwards)
    let stages: [(&str, &str, bool); 5] = [
        ("enrolled", "Active", true),
        ("opted out", "Inactive", false),
        ("enrolled again", "Active", true),
        ("still enrolled", "Active", true),
        ("opted out again", "Inactive", false),
    ];

    // The ids the catalog holds for this account, learned the first time they
    // are readable and then asserted against in every later stage.
    let mut catalog: Vec<String> = Vec::new();
    for (case, status, readable) in stages {
        ok(
            &router,
            "UpdateEnrollmentStatus",
            &json!({"status": status, "includeMemberAccounts": false}),
        )
        .await;

        if readable {
            // Poll the way a client would, bounded: an analysis that never
            // comes due fails the case instead of hanging it.
            for _ in 0..40 {
                if !listed(&router, true).await.is_empty() {
                    break;
                }
                std::thread::sleep(ANALYSIS_WAIT / 4);
            }
        }

        let served = ids(&ok(
            &router,
            "ListRecommendations",
            &json!({"includeAllRecommendations": true}),
        )
        .await);
        assert_eq!(
            !served.is_empty(),
            readable,
            "case {case:?}: expected rows readable = {readable}, got {served:?}"
        );
        if readable && catalog.is_empty() {
            catalog = served.clone();
        }
        if readable {
            assert_eq!(
                served, catalog,
                "case {case:?}: expected the same catalog the first readable stage served, got \
                 {served:?}"
            );
        }

        // Every id the list serves reads back by id, and every id it no longer
        // serves is one this caller holds no recommendation for.
        assert!(
            !catalog.is_empty(),
            "case {case:?}: expected the catalog ids to compare against"
        );
        for recommendation_id in &catalog {
            let (status, payload) = call(
                &router,
                "GetRecommendation",
                &json!({"recommendationId": recommendation_id}),
            )
            .await;
            let expected = if readable { 200 } else { 404 };
            assert_eq!(
                status, expected,
                "case {case:?} id {recommendation_id:?}: expected HTTP {expected}, got {status} \
                 with {payload}"
            );
            if readable {
                assert_eq!(
                    payload["recommendationId"].as_str(),
                    Some(recommendation_id.as_str()),
                    "case {case:?} id {recommendation_id:?}: expected the id asked for, got \
                     {payload}"
                );
            } else {
                assert_eq!(
                    error_code(&payload),
                    "ResourceNotFoundException",
                    "case {case:?} id {recommendation_id:?}: expected ResourceNotFoundException, \
                     got {payload}"
                );
            }
        }

        // The third reader moves with them: groups and a total only while the
        // rows are readable.
        let summary = ok(
            &router,
            "ListRecommendationSummaries",
            &json!({"groupBy": "Region"}),
        )
        .await;
        let groups = summary["items"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            !groups.is_empty(),
            readable,
            "case {case:?}: expected summary groups = {readable}, got {summary}"
        );
        let total = summary["estimatedTotalDedupedSavings"]
            .as_f64()
            .unwrap_or_default();
        assert_eq!(
            !same_money(total, 0.0),
            readable,
            "case {case:?}: expected a non-zero de-duped total = {readable}, got {total} in \
             {summary}"
        );
    }
}
