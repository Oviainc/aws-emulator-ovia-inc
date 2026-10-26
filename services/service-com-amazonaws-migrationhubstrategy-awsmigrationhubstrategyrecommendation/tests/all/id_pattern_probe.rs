// SPDX-License-Identifier: CC0-1.0

//! An id member probed character by character against the `@pattern` its model declares.
//!
//! The rule is the one `whole_value_patterns.rs` states over every pattern-bearing member:
//! a `@pattern` is a requirement on the WHOLE value, so a value carrying a character the
//! expression forbids is refused with 400 `ValidationException` and this service's one
//! invalid-request-body sentence before any lookup — even though the expression is *found*
//! in the legal part of the value, which is what Smithy's own reading of `@pattern` asks
//! for and what would otherwise carry such a value into a handler.
//!
//! This suite is the dense half of that rule: one member class — the ids the service's
//! operations take — probed with each character outside their expression at the START, the
//! MIDDLE and the END of an otherwise legal id, across every operation that takes one,
//! and then the same values against an operation whose handler MUTATES, to pin that the
//! refusal is decided before the mutation.

use serde_json::json;

use crate::import_and_assess::{call, emulator_with_collector, get_assessment, start_assessment};
use crate::whole_value_patterns::{ids_outside_the_whole_pattern, stop, INVALID_REQUEST_BODY};

/// The `AssessmentStatus` a stop moves a running assessment to, spelled out here so a
/// refused stop that nevertheless stopped the assessment fails this suite.
const ASSESSMENT_STOPPED: &str = "STOPPED";

/// The operations that take an id this probe can reach with an arbitrary spelling: the
/// assessment id in a body, the same id in a URI label, and the recommendation report
/// generation task id in a URI label.
///
/// `AsyncTaskId` (`StopAssessment$assessmentId`, `GetAssessment$id`) and
/// `RecommendationTaskId` (`GetRecommendationReportDetails$id`) declare the same
/// `[0-9a-z-:]+`, from
/// `smithy_query(query="shape",shape="AsyncTaskId,RecommendationTaskId")`, so one catalog
/// of spellings serves all three routes.
const ID_ROUTES: &[(&str, &str, &str)] = &[
    ("StopAssessment", "POST", "/stop-assessment"),
    ("GetAssessment", "GET", "/get-assessment/"),
    (
        "GetRecommendationReportDetails",
        "GET",
        "/get-recommendation-report-details/",
    ),
];

/// `value` with every character a URI path segment cannot carry literally percent-encoded,
/// so a row's id reaches the label exactly as the row spelled it.
///
/// Only the characters this suite's catalog uses are encoded; anything else would be a
/// silent change of the value under test, so an unexpected one panics the row rather than
/// being sent as something else.
fn as_label(value: &str) -> String {
    value
        .chars()
        .map(|character| match character {
            ' ' => "%20".to_string(),
            '/' => "%2F".to_string(),
            '\n' => "%0A".to_string(),
            '\t' => "%09".to_string(),
            '?' | '#' | '%' => {
                panic!("this suite has no encoding for {character:?} in a URI label")
            }
            other => other.to_string(),
        })
        .collect()
}

/// Every operation that takes an id refuses one its expression does not match as a whole,
/// with the same status, the same error code and the same sentence — whichever character is
/// the offending one and wherever in the value it sits.
#[tokio::test]
async fn an_id_outside_its_whole_pattern_is_refused_on_every_route() {
    let router = emulator_with_collector();

    for (named, id) in ids_outside_the_whole_pattern() {
        for (operation, method, route) in ID_ROUTES {
            let case = format!("{operation} with {named} ({id:?})");
            let (uri, body) = if *method == "POST" {
                (route.to_string(), Some(stop(&id)))
            } else {
                (format!("{route}{}", as_label(&id)), None)
            };
            let answer = call(&router, method, &uri, body).await;
            assert_eq!(
                answer.status, 400,
                "case {case:?}: expected HTTP 400, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {:?}",
                answer.body
            );
            assert_eq!(
                answer.string("/message"),
                INVALID_REQUEST_BODY,
                "case {case:?}: expected the service's {INVALID_REQUEST_BODY:?}, got {:?}",
                answer.body
            );
        }
    }
}

/// The refusal is decided BEFORE the lookup and the write: an assessment that exists is
/// untouched by a `StopAssessment` whose `assessmentId` is outside the expression, and the
/// same call with the assessment's own id stops it.
#[tokio::test]
async fn a_refused_stop_leaves_the_assessment_it_could_not_name_alone() {
    let router = emulator_with_collector();
    let started = start_assessment(
        &router,
        json!({"s3bucketForAnalysisData": "migrationhub-strategy-analysis"}),
    )
    .await;
    assert_eq!(
        started.status, 200,
        "the assessment that sets this case up must start, got {} with {:?}",
        started.status, started.body
    );
    let id = started.string("/assessmentId");

    for (named, malformed) in ids_outside_the_whole_pattern() {
        let refused = call(&router, "POST", "/stop-assessment", Some(stop(&malformed))).await;
        assert_eq!(
            refused.status, 400,
            "case {named:?}: expected HTTP 400, got {} with {:?}",
            refused.status, refused.body
        );
        let reported = get_assessment(&router, &id).await;
        assert_eq!(
            reported.status, 200,
            "case {named:?}: the assessment that exists is still reported, got {} with {:?}",
            reported.status, reported.body
        );
        assert_ne!(
            reported.string("/dataCollectionDetails/status"),
            ASSESSMENT_STOPPED,
            "case {named:?}: a refused stop stopped the assessment anyway: {:?}",
            reported.body
        );
    }

    let stopped = call(&router, "POST", "/stop-assessment", Some(stop(&id))).await;
    assert_eq!(
        stopped.status, 200,
        "the assessment's own id must still stop it, got {} with {:?}",
        stopped.status, stopped.body
    );
    assert_eq!(
        get_assessment(&router, &id)
            .await
            .string("/dataCollectionDetails/status"),
        ASSESSMENT_STOPPED,
        "an accepted stop must be reported on the assessment it named"
    );
}
