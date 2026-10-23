// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateFindingsFeedback`.
//!
//! Every request goes over the wire through `MultiServiceRouter::dispatch`, reusing the
//! harness in [`super::detector_lifecycle`], so routing, restJson1 (de)serialization,
//! modelled-constraint validation and the handlers are exercised together.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! call:
//!
//! 1. `UpdateFindingsFeedback` records the submitted `Feedback` on exactly the findings
//!    the request names, and `GetFindings` reports it as `Service.UserFeedback`; a
//!    later submission REPLACES it, so a read reports the latest value.
//! 2. `Comments` is accepted in any text form the member allows and is reported by no
//!    read: the member is `@sensitive` and the `Service` shape has no member for it.
//! 3. Recording feedback changes nothing else about the finding — not its
//!    `UpdatedAt`/`EventLastSeen` (the activity clock), not its archived flag, not the
//!    findings the request did not name.
//! 4. A `findingIds` entry the detector does not hold is not an error and writes
//!    nothing, the same reading of an unheld finding id that `GetFindings` takes.
//! 5. A detector id this account and Region do not hold is refused with HTTP 400 under
//!    the `InvalidInputException` class, and the refused request writes nothing.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_guardduty_guarddutyapiservice::ops::types::shared_types::FEEDBACK_VALUES;

use super::client_rejections::{invalid_parameter, not_owned};
use super::detector_lifecycle::{
    all_finding_ids, call, call_in, enabled_detector, error_code, error_shape, new_router,
    Router, ABSENT_DETECTOR_ID,
};

/// Finding types used to populate a detector. Any catalogued type will do — feedback
/// is about the finding, not about its type — so these span resource families.
const FINDING_TYPES: [&str; 4] = [
    "Recon:EC2/Portscan",
    "Impact:S3/TorIPCaller",
    "Policy:IAMUser/RootCredentialUsage",
    "Execution:Kubernetes/ExecInKubeSystemPod",
];

/// A value that cannot be a detector id at all.
const MALFORMED_DETECTOR_ID: &str = "not-a-detector-id";

/// Forms a `Comments` value can take. `SensitiveString` is a plain `@sensitive`
/// string with no length, pattern or alphabet constraint
/// (`smithy_query(shape="SensitiveString")`), so every row below is a legal value;
/// they differ along script, normalisation form, grapheme clustering, direction and
/// size.
fn comment_cases() -> Vec<(&'static str, Option<String>)> {
    vec![
        ("absent", None),
        ("ascii", Some("matches a real detection we triaged".to_string())),
        ("japanese", Some("この検出結果は調査に役立ちました".to_string())),
        ("emoji with zwj and modifier", Some("useful 👍🏽 👩‍💻".to_string())),
        ("arabic, right to left", Some("هذه النتيجة مفيدة جدا".to_string())),
        // The same text in NFD: combining marks must survive untouched.
        ("combining marks", Some("de\u{0301}ja\u{0300} vu".to_string())),
        ("empty string", Some(String::new())),
        ("long text", Some("é".repeat(2048))),
    ]
}

/// Create one sample finding per entry of `types` and return their ids, in list order.
async fn populate(router: &Router, detector_id: &str, types: &[&str]) -> Vec<String> {
    let (status, body) = call(
        router,
        "POST",
        &format!("/detector/{detector_id}/findings/create"),
        Some(json!({"findingTypes": types})),
    )
    .await;
    assert_eq!(
        status, 200,
        "create-sample-findings {types:?}: expected 200, got {status} {body:?}"
    );
    let ids = all_finding_ids(router, detector_id, 50).await;
    assert_eq!(
        ids.len(),
        types.len(),
        "expected one finding per requested type, got {ids:?}"
    );
    ids
}

/// Submit feedback, returning the status and body of the call.
async fn update_feedback(
    router: &Router,
    detector_id: &str,
    finding_ids: &[String],
    feedback: &str,
    comments: Option<&str>,
) -> (u16, Value) {
    update_feedback_in(router, "us-east-1", detector_id, finding_ids, feedback, comments).await
}

/// [`update_feedback`], against a chosen Region's endpoint.
async fn update_feedback_in(
    router: &Router,
    region: &str,
    detector_id: &str,
    finding_ids: &[String],
    feedback: &str,
    comments: Option<&str>,
) -> (u16, Value) {
    let mut body = json!({"findingIds": finding_ids, "feedback": feedback});
    if let Some(comments) = comments {
        body["comments"] = json!(comments);
    }
    call_in(
        router,
        region,
        "POST",
        &format!("/detector/{detector_id}/findings/feedback"),
        Some(body),
    )
    .await
}

/// Read findings back by id, keyed by id.
async fn findings_by_id(
    router: &Router,
    detector_id: &str,
    ids: &[String],
) -> BTreeMap<String, Value> {
    let (status, body) = call(
        router,
        "POST",
        &format!("/detector/{detector_id}/findings/get"),
        Some(json!({"findingIds": ids})),
    )
    .await;
    assert_eq!(status, 200, "get-findings: expected 200, got {status} {body:?}");
    body["findings"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|finding| {
            (
                finding["id"].as_str().unwrap_or_default().to_string(),
                finding,
            )
        })
        .collect()
}

/// The feedback a read reports for one finding, as `Option` so "none submitted" is
/// distinguishable from a submitted value.
fn reported_feedback(finding: &Value) -> Option<&str> {
    finding["service"]["userFeedback"].as_str()
}

// ---------------------------------------------------------------------------
// Rules 1 + 2 + 3: the submitted value round-trips, the comments do not surface,
// and nothing else about the finding moves.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn submitted_feedback_round_trips_for_every_value_and_comment_form() {
    for feedback in FEEDBACK_VALUES.iter().copied() {
        for (label, comments) in comment_cases() {
            let case = format!("{feedback} with comments {label}");
            let router = new_router();
            let detector_id = enabled_detector(&router).await;
            let ids = populate(&router, &detector_id, &FINDING_TYPES[..1]).await;

            let before = findings_by_id(&router, &detector_id, &ids).await;
            let before = before
                .get(&ids[0])
                .unwrap_or_else(|| panic!("case {case:?}: the finding must be readable"));
            assert_eq!(
                reported_feedback(before),
                None,
                "case {case:?}: a finding nobody gave feedback on must report none, got {before:?}"
            );

            let (status, body) = update_feedback(
                &router,
                &detector_id,
                &ids,
                feedback,
                comments.as_deref(),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status} {body:?}"
            );

            let after = findings_by_id(&router, &detector_id, &ids).await;
            let after = after
                .get(&ids[0])
                .unwrap_or_else(|| panic!("case {case:?}: the finding must still be readable"));
            assert_eq!(
                reported_feedback(after),
                Some(feedback),
                "case {case:?}: expected the submitted feedback, got {after:?}"
            );

            // Rule 2: the comments are reported nowhere, on either response.
            if let Some(comments) = comments.as_deref().filter(|text| !text.is_empty()) {
                for (which, response) in [("the update response", &body), ("the read", after)] {
                    let rendered = response.to_string();
                    assert!(
                        !rendered.contains(comments),
                        "case {case:?}: {which} must not report the sensitive comments, got {rendered}"
                    );
                }
            }

            // Rule 3: feedback is not an observation of the activity, so the
            // finding's clocks and its archived flag do not move.
            for member in ["createdAt", "updatedAt", "severity", "type", "arn"] {
                assert_eq!(
                    after[member], before[member],
                    "case {case:?}: {member} must not change when feedback is recorded, got {:?} (was {:?})",
                    after[member], before[member]
                );
            }
            for member in ["eventFirstSeen", "eventLastSeen", "archived", "count"] {
                assert_eq!(
                    after["service"][member], before["service"][member],
                    "case {case:?}: service.{member} must not change when feedback is recorded, got {:?} (was {:?})",
                    after["service"][member], before["service"][member]
                );
            }
        }
    }
}

/// The feedback is one value per finding, not a log: the latest submission is what a
/// read reports. The rows walk a sequence of submissions that changes the value, keeps
/// it, and changes it back, each with a different comments form.
#[tokio::test]
async fn the_latest_submission_is_what_a_read_reports() {
    let submissions: [(&str, Option<&str>); 5] = [
        ("USEFUL", None),
        ("NOT_USEFUL", Some("false positive: our own scanner")),
        ("NOT_USEFUL", Some("まだ有用ではありません")),
        ("USEFUL", Some("")),
        ("NOT_USEFUL", None),
    ];

    let router = new_router();
    let detector_id = enabled_detector(&router).await;
    let ids = populate(&router, &detector_id, &FINDING_TYPES).await;

    for (index, (feedback, comments)) in submissions.iter().enumerate() {
        let case = format!("submission {index}: {feedback} with comments {comments:?}");
        let (status, body) =
            update_feedback(&router, &detector_id, &ids, feedback, *comments).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {body:?}"
        );

        let findings = findings_by_id(&router, &detector_id, &ids).await;
        for id in &ids {
            let finding = findings
                .get(id)
                .unwrap_or_else(|| panic!("case {case:?}: finding {id} must be readable"));
            assert_eq!(
                reported_feedback(finding),
                Some(*feedback),
                "case {case:?}: expected the latest value on finding {id}, got {finding:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 3: only the findings the request names are written.
// ---------------------------------------------------------------------------

/// The rows differ along WHICH subset of the detector's findings the request names —
/// the first, the last, a pair, all of them and none of them — and the findings
/// outside the subset keep the feedback they already had.
#[tokio::test]
async fn feedback_is_recorded_only_for_the_findings_the_request_names() {
    let selections: [(&str, &[usize]); 5] = [
        ("the first finding", &[0]),
        ("the last finding", &[3]),
        ("a pair", &[1, 2]),
        ("every finding", &[0, 1, 2, 3]),
        ("no finding at all", &[]),
    ];

    for (label, selection) in selections {
        let router = new_router();
        let detector_id = enabled_detector(&router).await;
        let ids = populate(&router, &detector_id, &FINDING_TYPES).await;

        // A baseline every finding shares, so the selective write below is visible as
        // a DIFFERENCE and not merely as a value appearing.
        let (status, body) = update_feedback(&router, &detector_id, &ids, "USEFUL", None).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the baseline submission must succeed, got {status} {body:?}"
        );

        let named: Vec<String> = selection.iter().map(|index| ids[*index].clone()).collect();
        let (status, body) =
            update_feedback(&router, &detector_id, &named, "NOT_USEFUL", Some("retriaged")).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected 200, got {status} {body:?}"
        );

        let findings = findings_by_id(&router, &detector_id, &ids).await;
        for (index, id) in ids.iter().enumerate() {
            let expected = if selection.contains(&index) {
                "NOT_USEFUL"
            } else {
                "USEFUL"
            };
            let finding = findings
                .get(id)
                .unwrap_or_else(|| panic!("case {label:?}: finding {id} must be readable"));
            assert_eq!(
                reported_feedback(finding),
                Some(expected),
                "case {label:?}: finding {index} expected {expected}, got {finding:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 4: a finding id the detector does not hold is ignored.
// ---------------------------------------------------------------------------

/// `GetFindings` leaves an id the detector does not hold out of its result rather than
/// refusing the request; the feedback write takes the same reading. The rows differ
/// along the FORM of the unheld id and along whether a held id travels with it.
#[tokio::test]
async fn an_unheld_finding_id_is_ignored_rather_than_refused() {
    // Unheld ids: never minted, a plausible 32-hex id, the shortest and the longest
    // the model allows (`FindingId` is `len 1..300`,
    // `smithy_query(shape="FindingId")`).
    let unheld: [(&str, String); 4] = [
        ("never minted", "ffffffffffffffffffffffffffffffff".to_string()),
        ("not an id form", "no-such-finding".to_string()),
        ("shortest the model allows", "x".to_string()),
        ("longest the model allows", "f".repeat(300)),
    ];

    for (label, bogus) in unheld {
        for with_held in [false, true] {
            let case = format!("{label} (with a held id: {with_held})");
            let router = new_router();
            let detector_id = enabled_detector(&router).await;
            let ids = populate(&router, &detector_id, &FINDING_TYPES[..2]).await;

            let mut named = vec![bogus.clone()];
            if with_held {
                named.push(ids[0].clone());
            }
            let (status, body) =
                update_feedback(&router, &detector_id, &named, "USEFUL", None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: an unheld finding id is not an error, got {status} {body:?}"
            );

            let findings = findings_by_id(&router, &detector_id, &ids).await;
            assert_eq!(
                findings.len(),
                ids.len(),
                "case {case:?}: the detector's findings must all still be readable, got {findings:?}"
            );
            let expected_first = with_held.then_some("USEFUL");
            assert_eq!(
                reported_feedback(&findings[&ids[0]]),
                expected_first,
                "case {case:?}: expected {expected_first:?} on the held finding, got {:?}",
                findings[&ids[0]]
            );
            assert_eq!(
                reported_feedback(&findings[&ids[1]]),
                None,
                "case {case:?}: a finding the request never named must report no feedback, got {:?}",
                findings[&ids[1]]
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The modelled `Feedback` enum is what the service accepts.
// ---------------------------------------------------------------------------

/// `Feedback` is a modelled enum (`FEEDBACK_VALUES`, from
/// `smithy_query(shape="Feedback")`), so a string outside it is refused at the
/// boundary and NOTHING is recorded — a near miss must not end up reported as
/// `Service.UserFeedback`. The rows differ along the way a string can fail to be one
/// of the values: another case, a leading/trailing space, a plausible invention, the
/// empty string, and a value of a different enum.
#[tokio::test]
async fn a_feedback_value_outside_the_modelled_enum_is_refused_and_writes_nothing() {
    let cases: [(&str, &str); 6] = [
        ("lower case", "useful"),
        ("mixed case", "Not_Useful"),
        ("trailing space", "USEFUL "),
        ("a plausible invention", "VERY_USEFUL"),
        ("the empty string", ""),
        ("a value of another enum", "ENABLED"),
    ];

    let router = new_router();
    let detector_id = enabled_detector(&router).await;
    let ids = populate(&router, &detector_id, &FINDING_TYPES[..1]).await;

    for (label, feedback) in cases {
        let (status, body) = update_feedback(&router, &detector_id, &ids, feedback, None).await;
        assert_ne!(
            status, 200,
            "case {label:?} ({feedback:?}): a value outside the enum must not be accepted, got {status} {body:?}"
        );
        assert_eq!(
            status, 400,
            "case {label:?} ({feedback:?}): expected 400, got {status} {body:?}"
        );

        let findings = findings_by_id(&router, &detector_id, &ids).await;
        let finding = findings
            .get(&ids[0])
            .unwrap_or_else(|| panic!("case {label:?}: the finding must be readable"));
        assert_eq!(
            reported_feedback(finding),
            None,
            "case {label:?} ({feedback:?}): a refused submission must write nothing, got {finding:?}"
        );
    }

    // And a value the enum DOES carry is still accepted, so the refusals above are
    // about the value and not about the request being rejected wholesale.
    let (status, body) = update_feedback(&router, &detector_id, &ids, "USEFUL", None).await;
    assert_eq!(status, 200, "a modelled value: expected 200, got {status} {body:?}");
}

// ---------------------------------------------------------------------------
// Rule 5: a detector id this account and Region do not hold is refused.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_detector_id_the_region_does_not_hold_is_refused_and_writes_nothing() {
    let home = "eu-central-1";
    let router = new_router();
    let (status, created) =
        call_in(&router, home, "POST", "/detector", Some(json!({"enable": true}))).await;
    assert_eq!(status, 200, "create-detector in {home}: got {status} {created:?}");
    let detector_id = created["detectorId"]
        .as_str()
        .unwrap_or_else(|| panic!("create-detector: expected a detectorId, got {created:?}"))
        .to_string();

    let (status, body) = call_in(
        &router,
        home,
        "POST",
        &format!("/detector/{detector_id}/findings/create"),
        Some(json!({"findingTypes": [FINDING_TYPES[0]]})),
    )
    .await;
    assert_eq!(status, 200, "create-sample-findings in {home}: got {status} {body:?}");
    let (status, listed) = call_in(
        &router,
        home,
        "POST",
        &format!("/detector/{detector_id}/findings"),
        Some(json!({})),
    )
    .await;
    assert_eq!(status, 200, "list-findings in {home}: got {status} {listed:?}");
    let ids: Vec<String> = listed["findingIds"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|id| id.as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(ids.len(), 1, "expected one finding, got {ids:?}");

    // (case, Region the call is signed for, detector id, the message the service
    // classifies it with): a value that cannot be a detector id is a PARAMETER
    // failure, a well-formed one this account and Region do not hold an OWNERSHIP
    // failure. The two messages come from the same helpers the other refusal suite
    // uses, so this operation cannot drift from the service's wording.
    let refusals: [(&str, &str, &str, String); 3] = [
        (
            "a value that cannot be a detector id",
            home,
            MALFORMED_DETECTOR_ID,
            invalid_parameter("detectorId"),
        ),
        (
            "a well-formed id no create minted",
            home,
            ABSENT_DETECTOR_ID,
            not_owned("detectorId"),
        ),
        (
            "this detector, from another Region's endpoint",
            "ap-southeast-2",
            detector_id.as_str(),
            not_owned("detectorId"),
        ),
    ];

    for (case, region, target, message) in refusals {
        let (status, body) =
            update_feedback_in(&router, region, target, &ids, "USEFUL", Some("comment")).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInputException",
            "case {case:?}: expected the error class in __type, got {body:?}"
        );
        assert_eq!(
            body["type"],
            json!("InvalidInputException"),
            "case {case:?}: expected the error class in the type member, got {body:?}"
        );
        assert_eq!(
            error_shape(&body),
            "BadRequestException",
            "case {case:?}: expected the modelled envelope shape, got {body:?}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(message.as_str()),
            "case {case:?}: expected the classified message, got {body:?}"
        );

        // The refused request wrote nothing: the finding still reports no feedback.
        let (status, read) = call_in(
            &router,
            home,
            "POST",
            &format!("/detector/{detector_id}/findings/get"),
            Some(json!({"findingIds": ids})),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: get-findings got {status} {read:?}");
        let finding = &read["findings"][0];
        assert_eq!(
            reported_feedback(finding),
            None,
            "case {case:?}: a refused submission must write nothing, got {finding:?}"
        );
    }
}
