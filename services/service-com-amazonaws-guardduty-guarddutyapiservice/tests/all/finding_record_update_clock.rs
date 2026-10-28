// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the clock a finding's own metadata writes move.
//!
//! Every request goes over the wire through `MultiServiceRouter::dispatch`, reusing the
//! harness in [`super::detector_lifecycle`], so routing, restJson1 (de)serialization
//! and the handlers are exercised together.
//!
//! The ONE rule under test, asserted over a TABLE of writes rather than one recorded
//! call: a write that changes a stored property of a finding records the instant of
//! that write as the finding's own `UpdatedAt` — "the time and date when the finding
//! was last updated"
//! (`smithy_query(query="doc",shape="Finding",member="UpdatedAt")`) — for exactly the
//! findings whose stored values it changes, and moves nothing for a write that changes
//! no stored value. The clock of the observed ACTIVITY is the separate
//! `Service.EventLastSeen`, "the last-seen timestamp of the activity that prompted
//! GuardDuty to generate this finding"
//! (`smithy_query(query="doc",shape="Service",member="EventLastSeen")`), so a metadata
//! write leaves it — and `CreatedAt`, `Service.EventFirstSeen` and `Service.Count` —
//! exactly where the last observation left them.
//!
//! The rows span the metadata writes a finding has (`UpdateFindingsFeedback` and both
//! directions of the archive write) and, for each, the ways a submission can change a
//! stored value or change none: a first value, a different value, a repeat, a change in
//! only the second stored member (`Comments`), dropping that member, an empty batch and
//! a batch naming only ids the detector does not hold.
//!
//! The observable consequence is also asserted: `ListFindings` ordered by `updatedAt`
//! reports the order the writes happened in.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::detector_lifecycle::{
    all_finding_ids, call, enabled_detector, new_router, Router,
};

/// The finding types each test's detector is populated with. Source: the finding types
/// this service's sample catalogue publishes (`CreateSampleFindings`). The rule is
/// about the record's clock, not about the type, so the rows only need the findings to
/// be distinguishable.
const SAMPLE_TYPES: [&str; 3] = [
    "Recon:EC2/Portscan",
    "Impact:S3/TorIPCaller",
    "Policy:IAMUser/RootCredentialUsage",
];

/// A finding id of the minted form that no `CreateSampleFindings` ever produced.
const UNHELD_FINDING_ID: &str = "ffffffffffffffffffffffffffffffff";

/// One metadata write a caller can make against a finding.
#[derive(Debug, Clone)]
enum Write {
    /// `UpdateFindingsFeedback` with this `feedback` value and these `comments`.
    Feedback(&'static str, Option<&'static str>),
    /// `ArchiveFindings` (true) or `UnarchiveFindings` (false).
    Archived(bool),
}

/// An enabled detector holding one finding per entry of [`SAMPLE_TYPES`], and its
/// finding ids in list order.
async fn detector_with_findings(router: &Router) -> (String, Vec<String>) {
    let detector_id = enabled_detector(router).await;
    let (status, body) = call(
        router,
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

/// Send one metadata write over the named ids, failing the test unless it succeeded.
async fn apply(router: &Router, detector_id: &str, ids: &[String], write: &Write) {
    let (path, body) = match write {
        Write::Feedback(feedback, comments) => {
            let mut body = json!({"findingIds": ids, "feedback": feedback});
            if let Some(comments) = comments {
                body["comments"] = json!(comments);
            }
            ("findings/feedback", body)
        }
        Write::Archived(true) => ("findings/archive", json!({"findingIds": ids})),
        Write::Archived(false) => ("findings/unarchive", json!({"findingIds": ids})),
    };
    let (status, response) = call(
        router,
        "POST",
        &format!("/detector/{detector_id}/{path}"),
        Some(body),
    )
    .await;
    assert_eq!(
        status, 200,
        "write {write:?} over {ids:?}: expected 200, got {status} {response:?}"
    );
}

/// Every finding of `detector_id`, read back through `GetFindings` and keyed by id.
async fn findings_by_id(router: &Router, detector_id: &str, ids: &[String]) -> BTreeMap<String, Value> {
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

/// A top-level timestamp of one finding, as the fixed-width ISO-8601 text the wire
/// carries — so text order is time order.
fn text(finding: &Value, member: &str) -> String {
    finding[member]
        .as_str()
        .unwrap_or_else(|| panic!("expected a string {member}, got {finding:?}"))
        .to_string()
}

/// The members that record the observed ACTIVITY rather than the record: a metadata
/// write must leave every one of them alone.
const ACTIVITY_MEMBERS: [&str; 3] = ["eventFirstSeen", "eventLastSeen", "count"];

/// Each row: a write, the subset of the detector's findings it NAMES, and the subset
/// whose stored values that write actually changes given the rows before it — the only
/// findings whose `UpdatedAt` may move.
#[tokio::test]
async fn a_metadata_write_moves_updated_at_exactly_when_it_changes_a_stored_value() {
    let router = new_router();
    let (detector_id, ids) = detector_with_findings(&router).await;

    // (case, write, the indices of `ids` the request names, the indices it changes)
    let rows: [(&str, Write, &[usize], &[usize]); 12] = [
        (
            "the first feedback value on one finding",
            Write::Feedback("USEFUL", None),
            &[0],
            &[0],
        ),
        (
            "the same feedback value again",
            Write::Feedback("USEFUL", None),
            &[0],
            &[],
        ),
        (
            "the same value with comments added",
            Write::Feedback("USEFUL", Some("matches a real detection we triaged")),
            &[0],
            &[0],
        ),
        (
            "the same value with the same comments",
            Write::Feedback("USEFUL", Some("matches a real detection we triaged")),
            &[0],
            &[],
        ),
        (
            "the same value with different comments",
            Write::Feedback("USEFUL", Some("この検出結果は調査に役立ちました")),
            &[0],
            &[0],
        ),
        (
            "the same value with the comments dropped",
            Write::Feedback("USEFUL", None),
            &[0],
            &[0],
        ),
        (
            "the other feedback value",
            Write::Feedback("NOT_USEFUL", None),
            &[0],
            &[0],
        ),
        // The first finding already carries exactly this submission, so the SAME
        // request changes the second finding and is a no-op for the first: the clock
        // is per finding, not per request.
        (
            "a feedback value on a finding that had none, beside one that had it",
            Write::Feedback("NOT_USEFUL", None),
            &[0, 1],
            &[1],
        ),
        (
            "archiving findings that were not archived",
            Write::Archived(true),
            &[0, 1, 2],
            &[0, 1, 2],
        ),
        (
            "archiving them again",
            Write::Archived(true),
            &[0, 1, 2],
            &[],
        ),
        (
            "unarchiving them",
            Write::Archived(false),
            &[0, 1, 2],
            &[0, 1, 2],
        ),
        (
            "a feedback submission naming no finding at all",
            Write::Feedback("USEFUL", Some("nobody is named")),
            &[],
            &[],
        ),
    ];

    for (case, write, selection, changed) in rows {
        let before = findings_by_id(&router, &detector_id, &ids).await;
        let named: Vec<String> = selection.iter().map(|index| ids[*index].clone()).collect();
        apply(&router, &detector_id, &named, &write).await;
        let after = findings_by_id(&router, &detector_id, &ids).await;

        for (index, id) in ids.iter().enumerate() {
            let (was, now) = (&before[id], &after[id]);
            // Only a finding this write changes a stored value of may move.
            if changed.contains(&index) {
                assert!(
                    text(now, "updatedAt") > text(was, "updatedAt"),
                    "case {case:?}: finding {index} must take the instant of the write as its updatedAt, got {} (was {})",
                    text(now, "updatedAt"),
                    text(was, "updatedAt")
                );
            } else {
                assert_eq!(
                    text(now, "updatedAt"),
                    text(was, "updatedAt"),
                    "case {case:?}: finding {index} changed no stored value, so its updatedAt must stay put, got {} (was {})",
                    text(now, "updatedAt"),
                    text(was, "updatedAt")
                );
            }
            assert_eq!(
                text(now, "createdAt"),
                text(was, "createdAt"),
                "case {case:?}: finding {index} must keep its createdAt, got {now:?}"
            );
            for member in ACTIVITY_MEMBERS {
                assert_eq!(
                    now["service"][member], was["service"][member],
                    "case {case:?}: finding {index} must keep service.{member}, got {:?} (was {:?})",
                    now["service"][member], was["service"][member]
                );
            }
        }
    }
}

/// A batch naming only ids the detector does not hold writes nothing, so it moves no
/// finding's clock — the rows differ along the FORM of the unheld id.
#[tokio::test]
async fn a_feedback_submission_naming_only_unheld_ids_moves_no_clock() {
    // `FindingId` is a plain string, `len 1..300`
    // (`smithy_query(shape="FindingId")`), so each row below is a legal value the
    // detector simply does not hold.
    let unheld: [(&str, String); 4] = [
        ("of the minted form", UNHELD_FINDING_ID.to_string()),
        ("not of the minted form", "no-such-finding".to_string()),
        ("the shortest the model allows", "x".to_string()),
        ("the longest the model allows", "f".repeat(300)),
    ];

    for (label, bogus) in unheld {
        let router = new_router();
        let (detector_id, ids) = detector_with_findings(&router).await;
        let before = findings_by_id(&router, &detector_id, &ids).await;

        apply(
            &router,
            &detector_id,
            std::slice::from_ref(&bogus),
            &Write::Feedback("USEFUL", Some("about nothing this detector holds")),
        )
        .await;

        let after = findings_by_id(&router, &detector_id, &ids).await;
        for (index, id) in ids.iter().enumerate() {
            assert_eq!(
                text(&after[id], "updatedAt"),
                text(&before[id], "updatedAt"),
                "case {label:?}: finding {index} was not named, so its updatedAt must stay put, got {} (was {})",
                text(&after[id], "updatedAt"),
                text(&before[id], "updatedAt")
            );
        }
    }
}

/// The clock the write moves is the one `findingCriteria`/`sortCriteria` order by, so a
/// client can see the change: `ListFindings` sorted on `updatedAt` reports the findings
/// in the order their metadata was last written. The rows are the two orderings.
#[tokio::test]
async fn list_findings_sorted_on_updated_at_reports_the_order_the_writes_happened_in() {
    let router = new_router();
    let (detector_id, ids) = detector_with_findings(&router).await;

    // Write each finding's metadata in a chosen order, each with a different kind of
    // write, so the expected ordering is not an artefact of one operation.
    let writes: [(usize, Write); 3] = [
        (2, Write::Feedback("USEFUL", None)),
        (0, Write::Archived(true)),
        (1, Write::Feedback("NOT_USEFUL", Some("false positive"))),
    ];
    for (index, write) in &writes {
        apply(&router, &detector_id, &[ids[*index].clone()], write).await;
    }
    let oldest_first: Vec<String> = writes
        .iter()
        .map(|(index, _)| ids[*index].clone())
        .collect();

    for order_by in ["ASC", "DESC"] {
        let (status, body) = call(
            &router,
            "POST",
            &format!("/detector/{detector_id}/findings"),
            Some(json!({"sortCriteria": {"attributeName": "updatedAt", "orderBy": order_by}})),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {order_by:?}: list-findings expected 200, got {status} {body:?}"
        );
        let listed: Vec<String> = body["findingIds"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|id| id.as_str().unwrap_or_default().to_string())
            .collect();
        let mut expected = oldest_first.clone();
        if order_by == "DESC" {
            expected.reverse();
        }
        assert_eq!(
            listed, expected,
            "case {order_by:?}: expected the findings in the order their metadata was written, got {listed:?}"
        );
    }
}
