// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests that `ArchiveFindings` and `UnarchiveFindings` are the two
//! directions of ONE rule, driven over the wire through `MultiServiceRouter::dispatch`
//! with the harness in [`super::detector_lifecycle`].
//!
//! The rule under test: setting a finding's archive state resolves the detector the
//! request's Region serves, then resolves EVERY named finding id against that detector,
//! and only then writes the requested state — so for one request the direction of the
//! write is the only thing that can differ between the two operations. Both answer the
//! same status, the same error shape, the same error class and the same message for
//! every input, a batch naming an id the detector does not hold is refused whole in
//! either direction, and a request refused in either direction writes nothing.
//!
//! Each assertion covers a row of a TABLE spanning the input classes: an accepted
//! batch (empty, one id, every id), a finding id that fails to resolve (never minted,
//! not of the minted form, a held id upper-cased, alone or beside held ids), and a
//! detector id that fails to resolve (never created, not of the minted form, the real
//! id through another Region's endpoint) — including a request that fails BOTH ways,
//! which pins which of the two refusals comes first.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::client_rejections::{invalid_parameter, not_owned, UNUSABLE_INPUT_VALUE};
use super::detector_lifecycle::{
    all_finding_ids, call_in, enabled_detector, error_code, error_shape, new_router, Router,
    ABSENT_DETECTOR_ID,
};
use service_com_amazonaws_guardduty_guarddutyapiservice::state::INVALID_FINDING_IDS_MESSAGE;

/// The Region the detector under test is created in: the Region the harness signs for
/// by default.
const HOME_REGION: &str = "us-east-1";

/// A Region whose endpoint therefore serves no detector at all.
const OTHER_REGION: &str = "eu-west-1";

/// The finding types the detector under test is populated with. Source: the finding
/// types this service's sample catalogue publishes (`CreateSampleFindings`). The rule
/// is about the archive write, not about the type, so the rows only need the findings
/// to be distinguishable.
const SAMPLE_TYPES: [&str; 4] = [
    "Backdoor:EC2/DenialOfService.Tcp",
    "Discovery:S3/MaliciousIPCaller",
    "Execution:Runtime/ReverseShell",
    "Policy:Kubernetes/ExposedDashboard",
];

/// What a row expects: the request is accepted, or refused with this exact message.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Expected {
    Accepted,
    RefusedWith(String),
}

/// One request to either direction of the write, as a status and parsed body.
async fn set_archived_in(
    router: &Router,
    region: &str,
    detector_id: &str,
    ids: &[String],
    archived: bool,
) -> (u16, Value) {
    let path = if archived { "archive" } else { "unarchive" };
    call_in(
        router,
        region,
        "POST",
        &format!("/detector/{detector_id}/findings/{path}"),
        Some(json!({"findingIds": ids})),
    )
    .await
}

/// Each finding the detector holds and whether it is archived, read back through
/// `GetFindings`.
async fn archived_by_id(router: &Router, detector_id: &str) -> BTreeMap<String, bool> {
    let ids = all_finding_ids(router, detector_id, 50).await;
    let (status, body) = call_in(
        router,
        HOME_REGION,
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
        .iter()
        .map(|finding| {
            let id = finding["id"].as_str().unwrap_or_default().to_string();
            let archived = finding["service"]["archived"]
                .as_bool()
                .unwrap_or_else(|| panic!("finding {id}: expected a boolean archived, got {finding:?}"));
            (id, archived)
        })
        .collect()
}

/// An enabled detector holding one finding per entry of [`SAMPLE_TYPES`], and its
/// finding ids in list order.
async fn detector_with_findings(router: &Router) -> (String, Vec<String>) {
    let detector_id = enabled_detector(router).await;
    let (status, body) = call_in(
        router,
        HOME_REGION,
        "POST",
        &format!("/detector/{detector_id}/findings/create"),
        Some(json!({"findingTypes": SAMPLE_TYPES})),
    )
    .await;
    assert_eq!(
        status, 200,
        "create-sample-findings: expected 200, got {status} {body:?}"
    );
    let ids = all_finding_ids(router, &detector_id, 50).await;
    assert_eq!(
        ids.len(),
        SAMPLE_TYPES.len(),
        "expected one finding per sample type, got {ids:?}"
    );
    (detector_id, ids)
}

/// Assert one refusal: the modelled envelope, the invalid-input class, the message the
/// refused class carries, and no echo of the request's value.
fn assert_refusal(case: &str, expected_message: &str, value: &str, status: u16, body: &Value) {
    assert_eq!(status, 400, "case {case:?}: expected 400, got {status} {body:?}");
    assert_eq!(
        error_shape(body),
        "BadRequestException",
        "case {case:?}: expected the modelled error shape, got {body:?}"
    );
    assert_eq!(
        error_code(body),
        "InvalidInputException",
        "case {case:?}: expected the invalid-input class, got {body:?}"
    );
    let message = body["message"].as_str().unwrap_or_default();
    assert_eq!(
        message, expected_message,
        "case {case:?}: expected message {expected_message:?}, got {message:?}"
    );
    assert!(
        !message.contains(value),
        "case {case:?}: the refusal must not echo the request's value, got {message:?}"
    );
}

/// Both directions of the write answer the SAME verdict for the same request, and a
/// refused request in either direction leaves every finding's archive state where it
/// was.
#[tokio::test]
async fn both_directions_of_the_archive_write_follow_one_rule() {
    let router = new_router();
    let (detector_id, held) = detector_with_findings(&router).await;

    // A MIXED starting state, so a refused request that wrote in either direction
    // would show up in the snapshot comparison below.
    let (status, body) = set_archived_in(&router, HOME_REGION, &detector_id, &held[..2], true).await;
    assert_eq!(
        status, 200,
        "archiving the first two findings: expected 200, got {status} {body:?}"
    );

    let upper_cased_detector = detector_id.to_uppercase();
    let upper_cased_finding = held[0].to_uppercase();
    let rows: [(&str, &str, &str, Vec<String>, Expected); 12] = [
        (
            "an empty batch",
            HOME_REGION,
            detector_id.as_str(),
            vec![],
            Expected::Accepted,
        ),
        (
            "one held id",
            HOME_REGION,
            detector_id.as_str(),
            vec![held[0].clone()],
            Expected::Accepted,
        ),
        (
            "every held id",
            HOME_REGION,
            detector_id.as_str(),
            held.clone(),
            Expected::Accepted,
        ),
        (
            "a finding id nobody minted, alone",
            HOME_REGION,
            detector_id.as_str(),
            vec!["ffffffffffffffffffffffffffffffff".to_string()],
            Expected::RefusedWith(INVALID_FINDING_IDS_MESSAGE.to_string()),
        ),
        (
            "a finding id nobody minted, after held ids",
            HOME_REGION,
            detector_id.as_str(),
            vec![
                held[1].clone(),
                held[2].clone(),
                "ffffffffffffffffffffffffffffffff".to_string(),
            ],
            Expected::RefusedWith(INVALID_FINDING_IDS_MESSAGE.to_string()),
        ),
        (
            "a value that cannot be a finding id, before a held id",
            HOME_REGION,
            detector_id.as_str(),
            vec!["not-a-finding-id".to_string(), held[3].clone()],
            Expected::RefusedWith(UNUSABLE_INPUT_VALUE.to_string()),
        ),
        (
            "a held finding id upper-cased",
            HOME_REGION,
            detector_id.as_str(),
            vec![upper_cased_finding.clone()],
            Expected::RefusedWith(INVALID_FINDING_IDS_MESSAGE.to_string()),
        ),
        (
            "a detector id nobody created",
            HOME_REGION,
            ABSENT_DETECTOR_ID,
            held.clone(),
            Expected::RefusedWith(not_owned("detectorId")),
        ),
        (
            "a value that cannot be a detector id",
            HOME_REGION,
            "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz",
            held.clone(),
            Expected::RefusedWith(invalid_parameter("detectorId")),
        ),
        (
            "the detector through another Region's endpoint",
            OTHER_REGION,
            detector_id.as_str(),
            held.clone(),
            Expected::RefusedWith(not_owned("detectorId")),
        ),
        // A WELL-FORMED id the detector does not hold is resolved against the store,
        // so the detector is resolved first and a request that fails both ways is
        // refused on the detector id.
        (
            "a detector id nobody created AND an unheld finding id",
            HOME_REGION,
            upper_cased_detector.as_str(),
            vec!["ffffffffffffffffffffffffffffffff".to_string()],
            Expected::RefusedWith(not_owned("detectorId")),
        ),
        // A value that could not be a finding id at ALL is refused on its FORM, which
        // is settled before the store is consulted — the same precedence every pure
        // form rule on this service has (`CreateFilter` refuses a name outside the
        // filter-name alphabet before it resolves the detector) — so this request,
        // which also names a detector nobody created, is refused on the id.
        (
            "a detector id nobody created AND a value that cannot be a finding id",
            HOME_REGION,
            upper_cased_detector.as_str(),
            vec!["not-a-finding-id".to_string()],
            Expected::RefusedWith(UNUSABLE_INPUT_VALUE.to_string()),
        ),
    ];

    for row in rows {
        let (label, region, request_detector_id, ids, expected) = row;
        let before = archived_by_id(&router, &detector_id).await;

        let (archive_status, archive_body) =
            set_archived_in(&router, region, request_detector_id, &ids, true).await;
        let (unarchive_status, unarchive_body) =
            set_archived_in(&router, region, request_detector_id, &ids, false).await;

        // The two directions differ only in what they WRITE, so the verdict — status,
        // error shape, error class and message — is the same for both.
        assert_eq!(
            archive_status, unarchive_status,
            "case {label:?}: both directions must answer one status, got {archive_status} and {unarchive_status}"
        );
        assert_eq!(
            error_shape(&archive_body),
            error_shape(&unarchive_body),
            "case {label:?}: both directions must answer one error shape, got {archive_body:?} and {unarchive_body:?}"
        );
        assert_eq!(
            error_code(&archive_body),
            error_code(&unarchive_body),
            "case {label:?}: both directions must answer one error class, got {archive_body:?} and {unarchive_body:?}"
        );
        assert_eq!(
            archive_body["message"], unarchive_body["message"],
            "case {label:?}: both directions must answer one message, got {:?} and {:?}",
            archive_body["message"], unarchive_body["message"]
        );

        match expected {
            Expected::Accepted => {
                assert_eq!(
                    archive_status, 200,
                    "case {label:?}: expected 200, got {archive_status} {archive_body:?}"
                );
                // The accepted pair archived the named ids and then unarchived them,
                // so the detector ends where it started apart from those ids, which
                // the LAST call left unarchived.
                let after = archived_by_id(&router, &detector_id).await;
                for (id, was) in &before {
                    let expected_state = if ids.contains(id) { false } else { *was };
                    assert_eq!(
                        after.get(id).copied(),
                        Some(expected_state),
                        "case {label:?}: finding {id} expected archived={expected_state}, got {:?}",
                        after.get(id)
                    );
                }
                // Restore the mixed starting state for the rows that follow.
                let (status, body) =
                    set_archived_in(&router, HOME_REGION, &detector_id, &held[..2], true).await;
                assert_eq!(
                    status, 200,
                    "case {label:?}: restoring the starting state expected 200, got {status} {body:?}"
                );
            }
            Expected::RefusedWith(expected_message) => {
                // The request's own value, which no refusal may echo: the detector id
                // when that is what failed to resolve, otherwise the first named
                // finding id the detector does not hold.
                let value = if expected_message.contains("detectorId") {
                    request_detector_id.to_string()
                } else {
                    ids.iter()
                        .find(|id| !held.contains(id))
                        .cloned()
                        .unwrap_or_default()
                };
                assert_refusal(
                    &format!("{label} (archive)"),
                    &expected_message,
                    &value,
                    archive_status,
                    &archive_body,
                );
                assert_refusal(
                    &format!("{label} (unarchive)"),
                    &expected_message,
                    &value,
                    unarchive_status,
                    &unarchive_body,
                );
                assert_eq!(
                    archived_by_id(&router, &detector_id).await,
                    before,
                    "case {label:?}: a refused request must move no finding's archive state"
                );
            }
        }
    }
}
