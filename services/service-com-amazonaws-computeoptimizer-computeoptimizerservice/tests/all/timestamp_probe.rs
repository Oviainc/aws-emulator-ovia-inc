// SPDX-License-Identifier: CC0-1.0

//! The instant this service reports for an analysis, probed over the wire.
//!
//! Every recommendation reader of this service reports a `lastRefreshTimestamp` and every
//! export file states a `LastRefreshTimestamp` column, and all of them mean ONE instant: when
//! the analysis the account's enrollment started was generated
//! (`state::Enrollment::last_analysis_refresh`). What is asserted here is that one instant's
//! invariants, which no single reader's suite can state:
//!
//! * every reader of the same analysis reports the SAME instant, so two readers cannot
//!   disagree about when it was generated;
//! * it is one analysis span after the enrollment record a client can read for itself, so the
//!   instant is derived from the store rather than read off the clock when the call arrives.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::state::ANALYSIS_DELAY;
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{call, emulator, opt_in};

/// The readers that report an instant for the analysis of the store this emulator starts
/// with, and the member of the response carrying their recommendations.
const READERS: [(&str, &str); 6] = [
    ("GetEC2InstanceRecommendations", "instanceRecommendations"),
    (
        "GetAutoScalingGroupRecommendations",
        "autoScalingGroupRecommendations",
    ),
    ("GetEBSVolumeRecommendations", "volumeRecommendations"),
    (
        "GetLambdaFunctionRecommendations",
        "lambdaFunctionRecommendations",
    ),
    ("GetLicenseRecommendations", "licenseRecommendations"),
    ("GetIdleRecommendations", "idleRecommendations"),
];

/// Opt the account in and let the analysis its enrollment starts come due. The span is the
/// service's own ([`ANALYSIS_DELAY`]); the extra margin covers scheduler jitter.
async fn analysed(router: &MultiServiceRouter) {
    opt_in(router).await;
    tokio::time::sleep(ANALYSIS_DELAY + Duration::from_millis(100)).await;
}

/// The instants `operation` reports for the recommendations it carries in `member`.
async fn reported_instants(
    router: &MultiServiceRouter,
    operation: &str,
    member: &str,
) -> Vec<f64> {
    let (status, body) = call(router, operation, json!({})).await;
    assert_eq!(
        status, 200,
        "{operation}: expected 200, got {status}: {body}"
    );
    let reported = body[member].as_array().cloned().unwrap_or_default();
    assert!(
        !reported.is_empty(),
        "{operation}: the analysis holds recommendations to report an instant for: {body}"
    );
    reported
        .iter()
        .map(|entry| {
            entry["lastRefreshTimestamp"].as_f64().unwrap_or_else(|| {
                panic!("{operation}: every recommendation states when it was generated: {entry}")
            })
        })
        .collect()
}

/// The instant the enrollment record a client can read for itself was written.
async fn enrollment_written(router: &MultiServiceRouter) -> f64 {
    let (status, body): (u16, Value) = call(router, "GetEnrollmentStatus", json!({})).await;
    assert_eq!(
        status, 200,
        "GetEnrollmentStatus: expected 200, got {status}: {body}"
    );
    body["lastUpdatedTimestamp"]
        .as_f64()
        .unwrap_or_else(|| panic!("the enrollment record states when it was written: {body}"))
}

/// The one instant every reader of this analysis reports, one analysis span after the
/// enrollment record was written.
#[tokio::test]
async fn one_analysis_has_one_refresh_instant_across_every_reader() {
    let router = emulator();
    analysed(&router).await;
    let written = enrollment_written(&router).await;

    let mut instants: Vec<(&str, f64)> = Vec::new();
    for (operation, member) in READERS {
        for instant in reported_instants(&router, operation, member).await {
            instants.push((operation, instant));
        }
    }

    let (first_operation, first) = instants
        .first()
        .copied()
        .expect("the readers report recommendations");
    for (operation, instant) in &instants {
        // The SAME instant, not one within a tolerance: every reader derives it from the one
        // stored enrollment record, so the numbers are bit-identical.
        assert_eq!(
            instant.to_bits(),
            first.to_bits(),
            "case {operation:?}: every reader of one analysis reports the instant \
             {first_operation:?} reports ({first}), got {instant}"
        );
        // One analysis span after the enrollment record the caller can read for itself: the
        // instant is derived from the store, not taken from the clock when a read arrives, so
        // it does not drift with the time between the enrollment and this call. The tolerance
        // is a millisecond, far below the time these reads take.
        let span = instant - written;
        let expected = ANALYSIS_DELAY.as_secs_f64();
        assert!(
            (span - expected).abs() <= 0.001,
            "case {operation:?}: expected the instant {expected} seconds after the enrollment \
             record was written, got a span of {span} seconds"
        );
    }
}

/// A second read of the same analysis reports the same instant: the instant is a function of
/// the stored enrollment record, so re-reading neither moves it nor re-derives it from the
/// clock.
#[tokio::test]
async fn re_reading_an_analysis_reports_the_instant_it_reported_before() {
    let router = emulator();
    analysed(&router).await;

    for (operation, member) in READERS {
        let before = reported_instants(&router, operation, member).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        let again = reported_instants(&router, operation, member).await;
        let bits = |instants: &[f64]| instants.iter().map(|at| at.to_bits()).collect::<Vec<u64>>();
        assert_eq!(
            bits(&again),
            bits(&before),
            "case {operation:?}: expected the instants {before:?} again, got {again:?}"
        );
    }
}
