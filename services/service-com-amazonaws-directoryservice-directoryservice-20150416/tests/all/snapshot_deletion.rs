// SPDX-License-Identifier: CC0-1.0

//! Deleting a Directory Service snapshot, driven end to end over HTTP.
//!
//! Every case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: DirectoryService_20150416.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router. The plumbing is the shared `harness` module,
//! so nothing here reaches into the state struct and a response that is not computed
//! from persisted state cannot pass.
//!
//! The cases assert the RULE over sets of inputs rather than one recorded call: a delete
//! drops exactly the one snapshot its `SnapshotId` resolves to and reports that id — over
//! a table of rows differing in the snapshot's position in creation order, whether it
//! carries a `Name`, which directory it was taken of and whether the clock-owed
//! `Creating` -> `Completed` transition has fallen due — and leaves every other snapshot
//! reportable, in creation order, through both `DescribeSnapshots` filters; the slot the
//! deleted snapshot held against the per-directory manual snapshot quota is free again,
//! as `GetSnapshotLimits` reports and a further `CreateSnapshot` shows; and a
//! `SnapshotId` resolving to no stored snapshot — never taken, already deleted, or
//! deleted with its directory — is refused with the modelled error and removes nothing.
//!
//! The last case covers the handle a client correlates such a refusal by: the request id
//! it states in its modelled member, repeats at the end of its message and reports in the
//! response header — one id per call, wherever a client reads it.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::directory_lifecycle::PAST_ONE_SPAN;
use crate::harness::{call, call_raw, ok, refused, service, string};

/// A password satisfying the modelled `Password` pattern, so the create requests these
/// cases need are well formed. It is never asserted on.
const PASSWORD: &str = "Passw0rd!2345";

/// The first value of the modelled `SnapshotStatus` enum: where a snapshot sits until
/// the transition its create owes falls due.
const STATUS_CREATING: &str = "Creating";

/// The `Completed` value of the modelled `SnapshotStatus` enum.
const STATUS_COMPLETED: &str = "Completed";

/// Directory Service's default quota "Manual snapshots: 5" per directory, the bound
/// `GetSnapshotLimits` reports as `SnapshotLimits$ManualSnapshotsLimit`.
const MANUAL_SNAPSHOT_QUOTA: usize = 5;

/// Well-formed `SnapshotId`s (`^s-[0-9a-f]{10}$`) that are never created: the all-zero
/// and all-f ends of the pattern's value space, plus two mixed spellings in between.
const ABSENT_SNAPSHOT_IDS: [&str; 4] = [
    "s-0000000000",
    "s-00000000ff",
    "s-9f8e7d6c5b",
    "s-ffffffffff",
];

/// The header a response states its own request id in, the one an AWS SDK surfaces as
/// the response metadata's request id.
const REQUEST_ID_HEADER: &str = "x-amzn-requestid";

/// `CreateDirectory` for `name`, returning the minted directory id.
async fn create_directory(router: &MultiServiceRouter, name: &str, case: &str) -> String {
    let answer = ok(
        router,
        "CreateDirectory",
        json!({ "Name": name, "Password": PASSWORD, "Size": "Small" }),
        case,
    )
    .await;
    string(&answer, "DirectoryId", case)
}

/// `CreateSnapshot` of `directory_id`, with `name` when given, returning the minted
/// snapshot id.
async fn create_snapshot(
    router: &MultiServiceRouter,
    directory_id: &str,
    name: Option<&str>,
    case: &str,
) -> String {
    let mut body = json!({ "DirectoryId": directory_id });
    if let Some(name) = name {
        body["Name"] = json!(name);
    }
    let answer = ok(router, "CreateSnapshot", body, case).await;
    string(&answer, "SnapshotId", case)
}

/// `DescribeSnapshots` with `body`, returning the `SnapshotId`s it reports, in order.
async fn reported_ids(router: &MultiServiceRouter, body: Value, case: &str) -> Vec<String> {
    let answer = ok(router, "DescribeSnapshots", body, case).await;
    answer["Snapshots"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected Snapshots, got {answer}"))
        .iter()
        .map(|snapshot| string(snapshot, "SnapshotId", case))
        .collect()
}

/// The `Status` the store reports for one snapshot.
async fn status_of(router: &MultiServiceRouter, snapshot_id: &str, case: &str) -> String {
    let answer = ok(
        router,
        "DescribeSnapshots",
        json!({ "SnapshotIds": [snapshot_id] }),
        case,
    )
    .await;
    let snapshot = &answer["Snapshots"][0];
    string(snapshot, "Status", case)
}

/// `GetSnapshotLimits`' current manual snapshot count for one directory.
async fn manual_count(router: &MultiServiceRouter, directory_id: &str, case: &str) -> u64 {
    let answer = ok(
        router,
        "GetSnapshotLimits",
        json!({ "DirectoryId": directory_id }),
        case,
    )
    .await;
    answer["SnapshotLimits"]["ManualSnapshotsCurrentCount"]
        .as_u64()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected a ManualSnapshotsCurrentCount, got {answer}")
        })
}

/// The error message a refusal reports, whichever casing the envelope uses for it.
fn message(parsed: &Value) -> String {
    parsed["message"]
        .as_str()
        .or_else(|| parsed["Message"].as_str())
        .unwrap_or_default()
        .to_string()
}

/// One row of the deletion table: which of the seeded snapshots the delete names, and
/// whether the clock has already taken it past the transition its create owes.
struct DeletionCase {
    case: &'static str,
    /// Index into the seeded snapshot list.
    index: usize,
    /// Whether to wait out the owed transition before the delete, so the row deletes a
    /// `Completed` snapshot rather than one still being taken.
    await_transition: bool,
    /// The status the snapshot must report just before the delete.
    expected_status: &'static str,
}

#[tokio::test]
async fn a_delete_drops_exactly_the_snapshot_its_id_resolves_to() {
    let router = service();
    let first = create_directory(&router, "corp.example.com", "deletion").await;
    // A second directory with snapshots of its own, so a delete must not touch them.
    let second = create_directory(&router, "sales.example.org", "deletion").await;

    // The seeded set spans what a delete can resolve to: named and unnamed snapshots,
    // first/middle/last in creation order, and snapshots of two different directories.
    let seeded = [
        create_snapshot(&router, &first, Some("nightly"), "deletion").await,
        create_snapshot(&router, &first, None, "deletion").await,
        create_snapshot(&router, &second, Some("pre-upgrade @ 03:00"), "deletion").await,
        create_snapshot(&router, &first, Some("weekly"), "deletion").await,
        create_snapshot(&router, &second, None, "deletion").await,
    ];
    // Which directory each seeded snapshot was taken of, in the same order.
    let owners = [&first, &first, &second, &first, &second];
    let owner_of = |snapshot_id: &str| -> &String {
        let position = seeded
            .iter()
            .position(|id| id == snapshot_id)
            .expect("every id under test is one of the seeded snapshots");
        owners[position]
    };

    let cases = [
        DeletionCase {
            case: "the middle snapshot of its directory, still being taken",
            index: 1,
            await_transition: false,
            expected_status: STATUS_CREATING,
        },
        DeletionCase {
            case: "the first snapshot created, once the owed transition fell due",
            index: 0,
            await_transition: true,
            expected_status: STATUS_COMPLETED,
        },
        DeletionCase {
            case: "a named snapshot of the other directory",
            index: 2,
            await_transition: false,
            expected_status: STATUS_COMPLETED,
        },
        DeletionCase {
            case: "the last snapshot created, unnamed",
            index: 4,
            await_transition: false,
            expected_status: STATUS_COMPLETED,
        },
    ];

    let mut expected: Vec<String> = seeded.to_vec();
    for row in &cases {
        let case = row.case;
        let doomed = &seeded[row.index];
        if row.await_transition {
            tokio::time::sleep(PAST_ONE_SPAN).await;
        }
        let before = status_of(&router, doomed, case).await;
        assert_eq!(
            before, row.expected_status,
            "case {case:?}: expected the snapshot to report {:?} before the delete, got {before:?}",
            row.expected_status
        );

        // The delete reports the id of the snapshot it dropped.
        let answer = ok(
            &router,
            "DeleteSnapshot",
            json!({ "SnapshotId": doomed }),
            case,
        )
        .await;
        let reported = string(&answer, "SnapshotId", case);
        assert_eq!(
            &reported, doomed,
            "case {case:?}: expected DeleteSnapshot to report {doomed}, got {reported:?}"
        );

        // Exactly that snapshot is gone, and every other one is still reported, in
        // creation order.
        expected.retain(|id| id != doomed);
        let remaining = reported_ids(&router, json!({}), case).await;
        assert_eq!(
            remaining, expected,
            "case {case:?}: expected only {doomed} to be dropped, got {remaining:?}"
        );

        // The record is gone, not hidden: the `SnapshotIds` filter resolves nothing for
        // it, so the id is no longer a value that listing can be narrowed by.
        refused(
            &router,
            "DescribeSnapshots",
            json!({ "SnapshotIds": [doomed] }),
            "InvalidParameterException",
            case,
        )
        .await;

        // Each directory's own listing agrees with the unfiltered one.
        for directory in [&first, &second] {
            let per_directory =
                reported_ids(&router, json!({ "DirectoryId": directory }), case).await;
            let expected_here: Vec<String> = expected
                .iter()
                .filter(|id| owner_of(id) == directory)
                .cloned()
                .collect();
            assert_eq!(
                per_directory, expected_here,
                "case {case:?}: expected directory {directory}'s listing to be {expected_here:?}, \
                 got {per_directory:?}"
            );
        }
    }

    assert_eq!(
        expected.len(),
        seeded.len() - cases.len(),
        "expected one snapshot dropped per row, {expected:?} left of {seeded:?}"
    );
}

#[tokio::test]
async fn a_delete_frees_the_slot_it_held_against_the_per_directory_quota() {
    let router = service();
    let directory = create_directory(&router, "corp.example.com", "quota").await;
    let other = create_directory(&router, "sales.example.org", "quota").await;
    let untouched = create_snapshot(&router, &other, Some("untouched"), "quota").await;

    let mut held: Vec<String> = Vec::new();
    for index in 0..MANUAL_SNAPSHOT_QUOTA {
        let case = format!("snapshot {} of {MANUAL_SNAPSHOT_QUOTA}", index + 1);
        held.push(create_snapshot(&router, &directory, Some(&format!("snap_{index}")), &case).await);
    }
    assert_eq!(
        manual_count(&router, &directory, "at the quota").await,
        MANUAL_SNAPSHOT_QUOTA as u64,
        "expected the directory to hold its quota of manual snapshots"
    );
    refused(
        &router,
        "CreateSnapshot",
        json!({ "DirectoryId": &directory, "Name": "one_too_many" }),
        "SnapshotLimitExceededException",
        "one past the quota",
    )
    .await;

    // Each delete frees exactly one slot, so the reported count falls by one per call, a
    // create is accepted again as soon as it does, and the directory is at its quota
    // again once that create has taken the freed slot.
    let mut replacements: Vec<String> = Vec::new();
    for (index, doomed) in held.iter().enumerate() {
        let case = format!("freeing slot {}", index + 1);
        ok(
            &router,
            "DeleteSnapshot",
            json!({ "SnapshotId": doomed }),
            &case,
        )
        .await;
        let counted = manual_count(&router, &directory, &case).await;
        assert_eq!(
            counted,
            (MANUAL_SNAPSHOT_QUOTA - 1) as u64,
            "case {case:?}: expected the freed slot to be reported, got {counted}"
        );
        let replacement = create_snapshot(
            &router,
            &directory,
            Some(&format!("replacement_{index}")),
            &case,
        )
        .await;
        let counted = manual_count(&router, &directory, &case).await;
        assert_eq!(
            counted,
            MANUAL_SNAPSHOT_QUOTA as u64,
            "case {case:?}: expected the replacement to take the freed slot, got {counted}"
        );
        // ...and the directory is at its quota again, so one more is refused.
        refused(
            &router,
            "CreateSnapshot",
            json!({ "DirectoryId": &directory, "Name": "still_too_many" }),
            "SnapshotLimitExceededException",
            &case,
        )
        .await;
        replacements.push(replacement);
    }

    // Every slot the directory holds is deletable, down to an empty directory.
    for (index, replacement) in replacements.iter().enumerate() {
        let case = format!("draining slot {}", index + 1);
        ok(
            &router,
            "DeleteSnapshot",
            json!({ "SnapshotId": replacement }),
            &case,
        )
        .await;
        let counted = manual_count(&router, &directory, &case).await;
        assert_eq!(
            counted,
            (MANUAL_SNAPSHOT_QUOTA - index - 1) as u64,
            "case {case:?}: expected one slot fewer to be reported, got {counted}"
        );
    }
    let drained = reported_ids(&router, json!({ "DirectoryId": &directory }), "drained").await;
    assert!(
        drained.is_empty(),
        "expected the drained directory to hold no snapshot, got {drained:?}"
    );

    // The quota is per directory, and so is the delete: the other directory's snapshot is
    // untouched by any of it.
    let elsewhere = reported_ids(&router, json!({ "DirectoryId": &other }), "the other directory").await;
    assert_eq!(
        elsewhere,
        vec![untouched],
        "expected the other directory's snapshot to be untouched, got {elsewhere:?}"
    );
}

#[tokio::test]
async fn a_delete_resolving_to_no_stored_snapshot_is_refused_and_removes_nothing() {
    let router = service();
    let directory = create_directory(&router, "corp.example.com", "refusals").await;
    let doomed_directory = create_directory(&router, "doomed.example.org", "refusals").await;
    let kept = create_snapshot(&router, &directory, Some("kept"), "refusals").await;

    // A snapshot that was taken and then deleted, so a repeat of the delete resolves to
    // nothing.
    let already_deleted = create_snapshot(&router, &directory, Some("twice"), "refusals").await;
    ok(
        &router,
        "DeleteSnapshot",
        json!({ "SnapshotId": &already_deleted }),
        "refusals: the first delete",
    )
    .await;

    // A snapshot that went with the directory it was taken of, so its id resolves to
    // nothing either.
    let with_its_directory =
        create_snapshot(&router, &doomed_directory, Some("doomed"), "refusals").await;
    crate::harness::delete_directory_and_wait(
        &router,
        &doomed_directory,
        "refusals: delete the directory",
    )
    .await;

    let mut absent: Vec<String> = ABSENT_SNAPSHOT_IDS.iter().map(|id| id.to_string()).collect();
    absent.push(already_deleted);
    absent.push(with_its_directory);

    for snapshot_id in &absent {
        let case = format!("a delete naming {snapshot_id}");
        let body = json!({ "SnapshotId": snapshot_id });
        refused(
            &router,
            "DeleteSnapshot",
            body.clone(),
            "EntityDoesNotExistException",
            &case,
        )
        .await;

        // The refusal is built from the request: it names the snapshot the caller asked
        // about, in this service's lookup wording, so the request can be fixed from the
        // message alone.
        let (_, parsed) = call(&router, "DeleteSnapshot", body).await;
        let reported = message(&parsed);
        assert!(
            reported.starts_with(&format!("Snapshot: {snapshot_id} does not exist")),
            "case {case:?}: expected the message to name the requested snapshot, got {parsed}"
        );

        // Nothing was removed by the refused delete.
        let remaining = reported_ids(&router, json!({}), &case).await;
        assert_eq!(
            remaining,
            vec![kept.clone()],
            "case {case:?}: expected the refused delete to remove nothing, got {remaining:?}"
        );
    }

    // The one stored snapshot is still deletable after every refusal above.
    let answer = ok(
        &router,
        "DeleteSnapshot",
        json!({ "SnapshotId": &kept }),
        "the delete that is accepted",
    )
    .await;
    assert_eq!(
        string(&answer, "SnapshotId", "the delete that is accepted"),
        kept,
        "expected the accepted delete to report {kept}, got {answer}"
    );
    let remaining = reported_ids(&router, json!({}), "every snapshot deleted").await;
    assert!(
        remaining.is_empty(),
        "expected no snapshot left, got {remaining:?}"
    );
}

/// The modelled `RequestId` pattern
/// (`^([A-Fa-f0-9]{8}-[A-Fa-f0-9]{4}-[A-Fa-f0-9]{4}-[A-Fa-f0-9]{4}-[A-Fa-f0-9]{12})$`),
/// as an assertion.
fn assert_request_id(id: &str, case: &str) {
    let groups: Vec<usize> = id.split('-').map(str::len).collect();
    assert_eq!(
        groups,
        vec![8, 4, 4, 4, 12],
        "case {case:?}: expected a request id in the 8-4-4-4-12 form, got {id:?}"
    );
    assert!(
        id.chars().all(|c| c == '-' || c.is_ascii_hexdigit()),
        "case {case:?}: expected a hexadecimal request id, got {id:?}"
    );
}

#[tokio::test]
async fn a_refused_delete_reports_one_request_id_in_document_and_header() {
    let router = service();
    let directory = create_directory(&router, "corp.example.com", "request ids").await;
    let snapshot = create_snapshot(&router, &directory, Some("correlated"), "request ids").await;
    ok(
        &router,
        "DeleteSnapshot",
        json!({ "SnapshotId": &snapshot }),
        "request ids: the first delete",
    )
    .await;

    // Rows differing in why the delete resolves to nothing: an id already deleted, and
    // ids that were never taken. Each refusal must name its own call once — the modelled
    // member, the message suffix and the header carrying ONE id.
    let mut cases: Vec<String> = vec![snapshot];
    cases.extend(ABSENT_SNAPSHOT_IDS.iter().map(|id| id.to_string()));

    let mut seen: BTreeSet<String> = BTreeSet::new();
    for snapshot_id in &cases {
        let case = format!("a refused delete of {snapshot_id}");
        let response = call_raw(&router, "DeleteSnapshot", json!({ "SnapshotId": snapshot_id })).await;
        let status = response.status().as_u16();
        let header = response
            .headers()
            .get(REQUEST_ID_HEADER)
            .map(|value| value.to_str().expect("a textual header").to_string());
        let payload = response.into_body();
        let parsed: Value = serde_json::from_slice(&payload)
            .unwrap_or_else(|_| panic!("case {case:?}: expected a JSON body, got {payload:?}"));
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {parsed}"
        );

        let stated = parsed["RequestId"].as_str().unwrap_or_else(|| {
            panic!("case {case:?}: expected the modelled RequestId member, got {parsed}")
        });
        assert_request_id(stated, &case);
        assert_eq!(
            header.as_deref(),
            Some(stated),
            "case {case:?}: expected the header to carry the id the document states \
             ({stated}), got {header:?}"
        );
        let reported = message(&parsed);
        assert!(
            reported.ends_with(&format!(" : RequestId: {stated}")),
            "case {case:?}: expected the message to end with that same id, got {parsed}"
        );

        // One id names ONE call: no two refusals report the same one.
        assert!(
            seen.insert(stated.to_string()),
            "case {case:?}: expected a request id not yet reported, got {stated} after {seen:?}"
        );
    }
}
