// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for a GuardDuty finding's ARCHIVE state: `ArchiveFindings` and
//! `UnarchiveFindings`, the two directions of one write.
//!
//! Every request goes over the wire through `MultiServiceRouter::dispatch`, reusing the
//! harness in [`super::detector_lifecycle`], so routing, restJson1 (de)serialization and
//! the handlers are exercised together.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! call:
//!
//! 1. Setting the archive state is a write over the `findingIds` the request NAMES: a
//!    named finding the detector holds takes the requested state, every other finding
//!    keeps the state it had, and an id the detector does not hold moves nothing.
//! 2. The LAST call to land decides the state, so a repeated call is a no-op and
//!    archive/unarchive compose in any order.
//! 3. A finding's `updatedAt` moves when the call changes its archive state and stays
//!    put when the call changes nothing; `createdAt` and the activity's own clocks
//!    (`service.eventFirstSeen` / `service.eventLastSeen`) never move, because an
//!    archive is a change to the RECORD and not an observation of the activity.
//! 4. A `detectorId` this account/Region does not hold is refused with the modelled
//!    `BadRequestException` envelope under the `InvalidInputException` class, and the
//!    refused request writes nothing.
//! 5. The stored state is what a `ListFindings` criterion on `service.archived`
//!    selects on, in both directions.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use super::detector_lifecycle::{
    all_finding_ids, call, call_in, enabled_detector, error_code, error_shape, new_router, Router,
    ABSENT_DETECTOR_ID,
};

/// The finding types each test's detector is populated with. Source: the GuardDuty
/// finding-types reference (the types this service's sample catalogue publishes). The
/// rules under test are about the archive state, not about the type, so the rows span
/// unrelated resource families only to keep the findings distinguishable.
const SAMPLE_TYPES: [&str; 5] = [
    "Backdoor:EC2/DenialOfService.Tcp",
    "Discovery:S3/MaliciousIPCaller",
    "Execution:Runtime/ReverseShell",
    "PenTest:IAMUser/PentooLinux",
    "Policy:Kubernetes/ExposedDashboard",
];

/// A value of the modelled `FindingId` form (`len 1..300`,
/// `smithy_query(shape="FindingId")`) that no detector holds.
const ABSENT_FINDING_ID: &str = "ffffffffffffffffffffffffffffffff";

/// The Region whose endpoint does not serve the detector these tests create.
const OTHER_REGION: &str = "us-west-2";

/// Generate one sample finding per entry of `types`.
async fn create_samples(router: &Router, detector_id: &str, types: &[&str]) {
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
}

/// An enabled detector holding one finding per entry of [`SAMPLE_TYPES`], and that
/// detector's finding ids in list order.
async fn detector_with_findings(router: &Router) -> (String, Vec<String>) {
    let detector_id = enabled_detector(router).await;
    create_samples(router, &detector_id, &SAMPLE_TYPES).await;
    let ids = all_finding_ids(router, &detector_id, 50).await;
    assert_eq!(
        ids.len(),
        SAMPLE_TYPES.len(),
        "expected one finding per sample type, got {ids:?}"
    );
    (detector_id, ids)
}

/// Archive (`archived` true) or unarchive (`archived` false) the named ids.
async fn set_archived(
    router: &Router,
    detector_id: &str,
    ids: &[String],
    archived: bool,
) -> (u16, Value) {
    let path = if archived { "archive" } else { "unarchive" };
    call(
        router,
        "POST",
        &format!("/detector/{detector_id}/findings/{path}"),
        Some(json!({"findingIds": ids})),
    )
    .await
}

/// [`set_archived`], failing the test unless the call succeeded.
async fn set_archived_ok(router: &Router, detector_id: &str, ids: &[String], archived: bool) {
    let (status, body) = set_archived(router, detector_id, ids, archived).await;
    assert_eq!(
        status, 200,
        "set archived={archived} over {ids:?}: expected 200, got {status} {body:?}"
    );
}

/// Every finding the detector holds, read back through `GetFindings` and keyed by
/// finding id.
async fn findings_by_id(router: &Router, detector_id: &str) -> BTreeMap<String, Value> {
    let ids = all_finding_ids(router, detector_id, 50).await;
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
            let id = finding["id"].as_str().unwrap_or_default().to_string();
            (id, finding)
        })
        .collect()
}

/// Each finding's `service.archived`, keyed by finding id.
async fn archived_by_id(router: &Router, detector_id: &str) -> BTreeMap<String, bool> {
    findings_by_id(router, detector_id)
        .await
        .into_iter()
        .map(|(id, finding)| {
            let archived = finding["service"]["archived"].as_bool().unwrap_or_else(|| {
                panic!("finding {id}: expected a boolean service.archived, got {finding:?}")
            });
            (id, archived)
        })
        .collect()
}

/// One finding's member as text, for the timestamp members.
fn text(finding: &Value, member: &str) -> String {
    finding[member]
        .as_str()
        .unwrap_or_else(|| panic!("expected {member} as text, got {finding:?}"))
        .to_string()
}

/// The ids `ListFindings` selects with an equality criterion on `service.archived`.
async fn ids_with_archived(router: &Router, detector_id: &str, archived: bool) -> BTreeSet<String> {
    let (status, body) = call(
        router,
        "POST",
        &format!("/detector/{detector_id}/findings"),
        Some(json!({
            "findingCriteria": {
                "criterion": {"service.archived": {"equals": [archived.to_string()]}}
            }
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "list-findings filtered on archived={archived}: expected 200, got {status} {body:?}"
    );
    body["findingIds"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|id| id.as_str().map(str::to_string))
        .collect()
}

// ---------------------------------------------------------------------------
// Rule 1: the write covers exactly the named ids the detector holds.
// ---------------------------------------------------------------------------

/// `findingIds` is a LIST (`len 0..50`, `smithy_query(operation="ArchiveFindings")`), so
/// the call is a set operation over the ids it names. The rows differ along the size and
/// the content of that set: none, one, a proper subset, all of them, the same id twice,
/// and a request mixing in ids the detector does not hold.
#[tokio::test]
async fn archiving_moves_exactly_the_named_findings() {
    let cases: [(&str, &[usize], &[&str]); 7] = [
        ("no id named", &[], &[]),
        ("one id", &[0], &[]),
        ("a proper subset", &[0, 2], &[]),
        ("every finding", &[0, 1, 2, 3, 4], &[]),
        ("the same id twice", &[1, 1], &[]),
        ("one held id beside an absent one", &[3], &[ABSENT_FINDING_ID]),
        ("only ids the detector does not hold", &[], &[ABSENT_FINDING_ID, "not-a-finding-id"]),
    ];

    for case in cases {
        let (label, selected, absent) = case;
        let router = new_router();
        let (detector_id, ids) = detector_with_findings(&router).await;

        let mut named: Vec<String> = selected.iter().map(|index| ids[*index].clone()).collect();
        named.extend(absent.iter().map(|id| (*id).to_string()));

        let (status, body) = set_archived(&router, &detector_id, &named, true).await;
        assert_eq!(
            status, 200,
            "case {label:?}: archive expected 200, got {status} {body:?}"
        );

        let archived = archived_by_id(&router, &detector_id).await;
        assert_eq!(
            archived.len(),
            ids.len(),
            "case {label:?}: expected every finding to still be readable, got {archived:?}"
        );
        for (index, id) in ids.iter().enumerate() {
            let expected = selected.contains(&index);
            assert_eq!(
                archived.get(id).copied(),
                Some(expected),
                "case {label:?}: finding {index} expected archived={expected}, got {:?}",
                archived.get(id)
            );
        }

        // The same write in the other direction returns every named finding to the
        // unarchived state, leaving the rest where they were (all unarchived here).
        let (status, body) = set_archived(&router, &detector_id, &named, false).await;
        assert_eq!(
            status, 200,
            "case {label:?}: unarchive expected 200, got {status} {body:?}"
        );
        let archived = archived_by_id(&router, &detector_id).await;
        for (index, id) in ids.iter().enumerate() {
            assert_eq!(
                archived.get(id).copied(),
                Some(false),
                "case {label:?}: finding {index} expected archived=false after the unarchive, got {:?}",
                archived.get(id)
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 2: the last call to land decides the state.
// ---------------------------------------------------------------------------

/// Each row is a SEQUENCE of calls over the same findings (`true` archives, `false`
/// unarchives); the state afterwards is the direction of the last call, whatever came
/// before it. The rows span both starting directions, repeats of the same call and
/// alternations of increasing length.
#[tokio::test]
async fn the_last_call_decides_the_archived_state() {
    let cases: [(&str, &[bool]); 8] = [
        ("archive", &[true]),
        ("unarchive from the initial state", &[false]),
        ("archive then unarchive", &[true, false]),
        ("unarchive then archive", &[false, true]),
        ("archive twice", &[true, true]),
        ("unarchive twice", &[false, false]),
        ("archive, unarchive, archive", &[true, false, true]),
        ("archive, archive, unarchive, unarchive", &[true, true, false, false]),
    ];

    for case in cases {
        let (label, sequence) = case;
        let router = new_router();
        let (detector_id, ids) = detector_with_findings(&router).await;

        for archived in sequence {
            let (status, body) = set_archived(&router, &detector_id, &ids, *archived).await;
            assert_eq!(
                status, 200,
                "case {label:?}: a call in the sequence expected 200, got {status} {body:?}"
            );
        }

        let expected = *sequence.last().expect("each row names at least one call");
        let archived = archived_by_id(&router, &detector_id).await;
        for id in &ids {
            assert_eq!(
                archived.get(id).copied(),
                Some(expected),
                "case {label:?}: expected archived={expected} after the sequence, got {:?}",
                archived.get(id)
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 3: updatedAt moves on a change, and only on a change.
// ---------------------------------------------------------------------------

/// `UpdatedAt` is "the time and date when the finding was last updated"
/// (`smithy_query(shape="Finding",member="UpdatedAt")`), so a call that moves the
/// archive state moves it and a call that changes nothing leaves it alone. `CreatedAt`
/// never moves. The timestamps are fixed-width ISO-8601 text, so text order is time
/// order.
#[tokio::test]
async fn updated_at_moves_only_when_the_archived_state_changes() {
    let router = new_router();
    let (detector_id, ids) = detector_with_findings(&router).await;
    let before = findings_by_id(&router, &detector_id).await;

    set_archived_ok(&router, &detector_id, &ids, true).await;
    let archived = findings_by_id(&router, &detector_id).await;
    for id in &ids {
        let (was, now) = (&before[id], &archived[id]);
        assert!(
            text(now, "updatedAt") > text(was, "updatedAt"),
            "case {id:?}: archiving must move updatedAt, got {} then {}",
            text(was, "updatedAt"),
            text(now, "updatedAt")
        );
        assert_eq!(
            text(now, "createdAt"),
            text(was, "createdAt"),
            "case {id:?}: archiving must not move createdAt, got {now:?}"
        );
        // The archive state is a property of the RECORD, so the clocks of the
        // observed ACTIVITY stay put: nothing was detected by this call.
        for member in ["eventFirstSeen", "eventLastSeen"] {
            assert_eq!(
                now["service"][member], was["service"][member],
                "case {id:?}: archiving must not move service.{member}, got {:?} (was {:?})",
                now["service"][member], was["service"][member]
            );
        }
    }

    // A repeat of the same call changes nothing, so it moves no timestamp either.
    set_archived_ok(&router, &detector_id, &ids, true).await;
    let repeated = findings_by_id(&router, &detector_id).await;
    for id in &ids {
        assert_eq!(
            text(&repeated[id], "updatedAt"),
            text(&archived[id], "updatedAt"),
            "case {id:?}: a repeated archive must not move updatedAt, got {:?}",
            repeated[id]
        );
    }

    // The other direction IS a change, so it moves updatedAt again.
    set_archived_ok(&router, &detector_id, &ids, false).await;
    let unarchived = findings_by_id(&router, &detector_id).await;
    for id in &ids {
        assert!(
            text(&unarchived[id], "updatedAt") > text(&repeated[id], "updatedAt"),
            "case {id:?}: unarchiving must move updatedAt, got {} then {}",
            text(&repeated[id], "updatedAt"),
            text(&unarchived[id], "updatedAt")
        );
        assert_eq!(
            unarchived[id]["service"]["eventLastSeen"], before[id]["service"]["eventLastSeen"],
            "case {id:?}: unarchiving must not move service.eventLastSeen, got {:?} (was {:?})",
            unarchived[id]["service"]["eventLastSeen"], before[id]["service"]["eventLastSeen"]
        );
        assert_eq!(
            unarchived[id]["service"]["archived"],
            json!(false),
            "case {id:?}: expected archived=false, got {:?}",
            unarchived[id]
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 4: a detectorId this account/Region does not hold is refused.
// ---------------------------------------------------------------------------

/// A detector is a regional resource owned by one account, so a `detectorId` the
/// endpoint does not resolve is refused rather than silently accepted. The rows differ
/// along the ways an id can fail to resolve: a well-formed id nobody created, a value
/// that is not an id at all (not hexadecimal, too short, too long), the real id
/// upper-cased, and the real id through another Region's endpoint.
#[tokio::test]
async fn a_detector_id_the_endpoint_does_not_hold_is_refused_and_writes_nothing() {
    let router = new_router();
    let (detector_id, ids) = detector_with_findings(&router).await;
    // A MIXED starting state, so a refused request that wrote in either direction
    // would show up below.
    set_archived_ok(&router, &detector_id, &ids[..2], true).await;
    let before = archived_by_id(&router, &detector_id).await;

    let upper_cased = detector_id.to_uppercase();
    let cases: [(&str, &str, &str); 6] = [
        ("well formed but never created", ABSENT_DETECTOR_ID, "us-east-1"),
        ("not hexadecimal", "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz", "us-east-1"),
        ("too short", "abc123", "us-east-1"),
        ("too long", "0123456789abcdef0123456789abcdef0", "us-east-1"),
        ("the detector's id upper-cased", upper_cased.as_str(), "us-east-1"),
        ("the detector's id in another Region", detector_id.as_str(), OTHER_REGION),
    ];

    for case in cases {
        let (label, id, region) = case;
        for operation in ["archive", "unarchive"] {
            let (status, body) = call_in(
                &router,
                region,
                "POST",
                &format!("/detector/{id}/findings/{operation}"),
                Some(json!({"findingIds": ids})),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {label:?} ({operation}): expected 400, got {status} {body:?}"
            );
            assert_eq!(
                error_shape(&body),
                "BadRequestException",
                "case {label:?} ({operation}): expected the modelled error shape, got {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidInputException",
                "case {label:?} ({operation}): expected the invalid-input class in __type, got {body:?}"
            );
            assert_eq!(
                body["type"],
                json!("InvalidInputException"),
                "case {label:?} ({operation}): expected the invalid-input class, got {body:?}"
            );
            let message = body["message"].as_str().unwrap_or_default();
            assert!(
                message.contains("detectorId"),
                "case {label:?} ({operation}): expected a message naming the member, got {body:?}"
            );
            assert!(
                !message.contains(id),
                "case {label:?} ({operation}): the refusal must not echo the request's value, got {body:?}"
            );
        }
    }

    assert_eq!(
        archived_by_id(&router, &detector_id).await,
        before,
        "a refused request must leave every finding's archived state where it was"
    );
}

// ---------------------------------------------------------------------------
// Rule 5: the stored state is what a filtered list selects on.
// ---------------------------------------------------------------------------

/// `service.archived` is a finding attribute a `ListFindings` criterion can select on,
/// so the state these two operations write is the state the filter reads. The rows
/// differ in how much of the detector is archived: nothing, one finding, a proper
/// subset, everything.
#[tokio::test]
async fn a_filtered_list_selects_on_the_stored_archived_state() {
    let cases: [(&str, &[usize]); 4] = [
        ("nothing archived", &[]),
        ("one archived", &[2]),
        ("a proper subset archived", &[0, 1, 3]),
        ("everything archived", &[0, 1, 2, 3, 4]),
    ];

    let router = new_router();
    let (detector_id, ids) = detector_with_findings(&router).await;

    for case in cases {
        let (label, selected) = case;
        // Start each row from the same known state.
        set_archived_ok(&router, &detector_id, &ids, false).await;
        let named: Vec<String> = selected.iter().map(|index| ids[*index].clone()).collect();
        set_archived_ok(&router, &detector_id, &named, true).await;

        let expected_archived: BTreeSet<String> = named.iter().cloned().collect();
        let expected_live: BTreeSet<String> = ids
            .iter()
            .filter(|id| !expected_archived.contains(*id))
            .cloned()
            .collect();

        let listed_archived = ids_with_archived(&router, &detector_id, true).await;
        assert_eq!(
            listed_archived, expected_archived,
            "case {label:?}: expected the archived ids, got {listed_archived:?}"
        );
        let listed_live = ids_with_archived(&router, &detector_id, false).await;
        assert_eq!(
            listed_live, expected_live,
            "case {label:?}: expected the unarchived ids, got {listed_live:?}"
        );
    }
}
