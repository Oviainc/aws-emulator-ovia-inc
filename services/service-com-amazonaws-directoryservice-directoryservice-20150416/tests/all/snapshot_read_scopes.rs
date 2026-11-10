// SPDX-License-Identifier: CC0-1.0

//! The two read forms of `DescribeSnapshots`, driven end to end over HTTP.
//!
//! Every case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: DirectoryService_20150416.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router. The plumbing is the shared `harness` module, so
//! nothing here reaches into the state struct and a response that is not computed from
//! persisted state cannot pass.
//!
//! The RULE under test, asserted over a table rather than one recorded call. `SnapshotIds`
//! means something different to each of this operation's two read forms:
//!
//! * narrowed by a `DirectoryId` the service holds, the listing IS that directory's
//!   snapshots and the ids select within them, so an entry selecting none of them selects
//!   nothing: the page — empty when nothing is left — is served with HTTP 200;
//! * unscoped, the ids are the whole of what the request asks about, so an id no snapshot
//!   is stored under is a filter value the listing cannot be narrowed by: 400
//!   `InvalidParameterException`, naming that value.
//!
//! The rows differ along WHY an id selects nothing (never minted, deleted, deleted with its
//! directory, belonging to another directory), along how many such ids a request carries,
//! and along whether they arrive beside ids that do select.
//!
//! The second case walks the documented snapshot workflow to its end — take a snapshot,
//! poll it to `Completed`, delete it, confirm through the directory's own listing that it
//! is gone, and re-delete it — over a table of snapshots differing in name, directory and
//! creation order.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The one wait the crate's suites share, derived from the spans the service publishes.
use crate::directory_lifecycle::PAST_ONE_SPAN;
use crate::harness::{call, ok, refused, service, string};

/// A password satisfying the modelled `Password` pattern, so the creates these cases need
/// are well formed. It is never asserted on.
const PASSWORD: &str = "Passw0rd!2345";

/// The `Completed` value of the modelled `SnapshotStatus` enum
/// (`smithy_query(query="shape", shape="SnapshotStatus")`).
const STATUS_COMPLETED: &str = "Completed";

/// Well-formed `SnapshotId`s (the modelled pattern `^s-[0-9a-f]{10}$`,
/// `smithy_query(query="operation", operation="DescribeSnapshots")`) that are never
/// minted: both ends of the pattern's value space and a mixed spelling in between.
const NEVER_MINTED: [&str; 3] = ["s-0000000000", "s-9f8e7d6c5b", "s-ffffffffff"];

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

/// The `SnapshotId`s a successful `DescribeSnapshots` reports, in the order it reports
/// them.
async fn reported_ids(router: &MultiServiceRouter, body: Value, case: &str) -> Vec<String> {
    let answer = ok(router, "DescribeSnapshots", body, case).await;
    answer["Snapshots"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected Snapshots, got {answer}"))
        .iter()
        .map(|snapshot| string(snapshot, "SnapshotId", case))
        .collect()
}

/// The `Message` of a parsed refusal, without the request-id suffix every refusal of this
/// service closes with.
fn stated(parsed: &Value, case: &str) -> String {
    let message = parsed["Message"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a Message, got {parsed}"));
    message
        .rsplit_once(" : RequestId: ")
        .map(|(text, _)| text)
        .unwrap_or(message)
        .to_string()
}

/// One row of the filter table: the ids the request carries, which of them the unscoped
/// read cannot resolve, and the page the directory-scoped read must serve for them.
struct Row {
    case: &'static str,
    /// The `SnapshotIds` the request carries, as indexes into the seeded ids plus the
    /// never-minted literals: built per row below.
    filter: Vec<String>,
    /// The ids the scoped read must report, in creation order.
    scoped_page: Vec<String>,
    /// The ids the unscoped read cannot resolve; empty when that read is served instead.
    unresolvable: Vec<String>,
    /// The ids the unscoped read must report when it is served, in creation order.
    unscoped_page: Vec<String>,
}

#[tokio::test]
async fn an_id_that_selects_nothing_is_a_page_when_scoped_and_a_parameter_when_not() {
    let router = service();
    let directory = create_directory(&router, "corp.example.com", "read scopes").await;
    let other = create_directory(&router, "sales.example.org", "read scopes").await;
    let doomed = create_directory(&router, "doomed.example.net", "read scopes").await;

    // Two snapshots that stay, one of another directory, one that is deleted, and one that
    // goes with the directory it was taken of: the four reasons an id can select nothing.
    let kept_first = create_snapshot(&router, &directory, Some("nightly"), "read scopes").await;
    let kept_second = create_snapshot(&router, &directory, None, "read scopes").await;
    let elsewhere = create_snapshot(&router, &other, Some("theirs"), "read scopes").await;
    let deleted = create_snapshot(&router, &directory, Some("gone"), "read scopes").await;
    let with_its_directory = create_snapshot(&router, &doomed, None, "read scopes").await;
    ok(
        &router,
        "DeleteSnapshot",
        json!({ "SnapshotId": &deleted }),
        "read scopes: the delete",
    )
    .await;
    crate::harness::delete_directory_and_wait(
        &router,
        &doomed,
        "read scopes: the directory delete",
    )
    .await;

    let rows = vec![
        Row {
            case: "an id nothing ever minted",
            filter: vec![NEVER_MINTED[0].to_string()],
            scoped_page: Vec::new(),
            unresolvable: vec![NEVER_MINTED[0].to_string()],
            unscoped_page: Vec::new(),
        },
        Row {
            case: "the other end of the id space",
            filter: vec![NEVER_MINTED[2].to_string()],
            scoped_page: Vec::new(),
            unresolvable: vec![NEVER_MINTED[2].to_string()],
            unscoped_page: Vec::new(),
        },
        Row {
            case: "a snapshot the caller has just deleted",
            filter: vec![deleted.clone()],
            scoped_page: Vec::new(),
            unresolvable: vec![deleted.clone()],
            unscoped_page: Vec::new(),
        },
        Row {
            case: "a snapshot that went with its directory",
            filter: vec![with_its_directory.clone()],
            scoped_page: Vec::new(),
            unresolvable: vec![with_its_directory.clone()],
            unscoped_page: Vec::new(),
        },
        Row {
            case: "three ids that select nothing at once",
            filter: vec![
                NEVER_MINTED[0].to_string(),
                NEVER_MINTED[1].to_string(),
                deleted.clone(),
            ],
            scoped_page: Vec::new(),
            unresolvable: vec![
                NEVER_MINTED[0].to_string(),
                NEVER_MINTED[1].to_string(),
                deleted.clone(),
            ],
            unscoped_page: Vec::new(),
        },
        Row {
            case: "an id that selects nothing beside one that does",
            filter: vec![kept_first.clone(), NEVER_MINTED[1].to_string()],
            scoped_page: vec![kept_first.clone()],
            unresolvable: vec![NEVER_MINTED[1].to_string()],
            unscoped_page: Vec::new(),
        },
        Row {
            case: "two stored ids and a deleted one, in reverse order",
            filter: vec![kept_second.clone(), kept_first.clone(), deleted.clone()],
            scoped_page: vec![kept_first.clone(), kept_second.clone()],
            unresolvable: vec![deleted.clone()],
            unscoped_page: Vec::new(),
        },
        Row {
            case: "another directory's snapshot",
            filter: vec![elsewhere.clone()],
            scoped_page: Vec::new(),
            unresolvable: Vec::new(),
            unscoped_page: vec![elsewhere.clone()],
        },
        Row {
            case: "another directory's snapshot beside one of this directory's",
            filter: vec![elsewhere.clone(), kept_second.clone()],
            scoped_page: vec![kept_second.clone()],
            unresolvable: Vec::new(),
            unscoped_page: vec![kept_second.clone(), elsewhere.clone()],
        },
        Row {
            case: "every stored snapshot of this directory",
            filter: vec![kept_first.clone(), kept_second.clone()],
            scoped_page: vec![kept_first.clone(), kept_second.clone()],
            unresolvable: Vec::new(),
            unscoped_page: vec![kept_first.clone(), kept_second.clone()],
        },
    ];

    for row in &rows {
        let case = row.case;

        // Scoped by the directory the caller is reading: a page, in creation order, of
        // exactly the snapshots the filters select — and never a refusal.
        let page = reported_ids(
            &router,
            json!({ "DirectoryId": &directory, "SnapshotIds": &row.filter }),
            case,
        )
        .await;
        assert_eq!(
            page, row.scoped_page,
            "case {case:?}: expected the scoped read to report {:?} in creation order, got {page:?}",
            row.scoped_page
        );

        // Unscoped, the same ids are the whole of the request: one that resolves to no
        // stored snapshot is the filter VALUE this listing cannot be narrowed by.
        if row.unresolvable.is_empty() {
            let page = reported_ids(&router, json!({ "SnapshotIds": &row.filter }), case).await;
            assert_eq!(
                page, row.unscoped_page,
                "case {case:?}: expected the unscoped read to report {:?}, got {page:?}",
                row.unscoped_page
            );
        } else {
            let (status, parsed) =
                call(&router, "DescribeSnapshots", json!({ "SnapshotIds": &row.filter })).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected the unscoped read to be refused, got {status} with {parsed}"
            );
            let wire_type = parsed["__type"].as_str().unwrap_or_default();
            assert!(
                wire_type.contains("InvalidParameterException"),
                "case {case:?}: expected InvalidParameterException, got {parsed}"
            );
            let message = stated(&parsed, case);
            let expected: Vec<String> = row
                .unresolvable
                .iter()
                .map(|id| format!("Invalid snapshot ID: {id}."))
                .collect();
            assert!(
                expected.contains(&message),
                "case {case:?}: expected one of {expected:?}, got {message:?}"
            );
        }
    }

    // The directory scope is still resolved: an id no directory is stored under is reported
    // rather than served as an empty page, so the page above is served because the
    // DIRECTORY is there.
    refused(
        &router,
        "DescribeSnapshots",
        json!({ "DirectoryId": &doomed, "SnapshotIds": [&with_its_directory] }),
        "EntityDoesNotExistException",
        "a directory that is gone",
    )
    .await;
}

/// One row of the lifecycle table: how the snapshot under test was taken.
struct Lifecycle {
    case: &'static str,
    /// Whether the snapshot carries a `Name`.
    name: Option<&'static str>,
    /// Whether it is taken of the second directory rather than the first.
    second_directory: bool,
}

#[tokio::test]
async fn the_snapshot_workflow_can_be_walked_to_its_end() {
    let router = service();
    let first = create_directory(&router, "corp.example.com", "lifecycle").await;
    let second = create_directory(&router, "sales.example.org", "lifecycle").await;
    // A snapshot that is never deleted, so each row's confirming read is about the row's
    // own snapshot and not about an empty store.
    let untouched = create_snapshot(&router, &first, Some("untouched"), "lifecycle").await;

    let rows = [
        Lifecycle {
            case: "a named snapshot of the first directory",
            name: Some("nightly"),
            second_directory: false,
        },
        Lifecycle {
            case: "an unnamed snapshot of the first directory",
            name: None,
            second_directory: false,
        },
        Lifecycle {
            case: "a named snapshot of the second directory",
            name: Some("pre-upgrade @ 03:00"),
            second_directory: true,
        },
    ];

    for row in rows {
        let case = row.case;
        let directory = if row.second_directory { &second } else { &first };
        let snapshot = create_snapshot(&router, directory, row.name, case).await;
        let scoped = json!({ "DirectoryId": directory, "SnapshotIds": [&snapshot] });

        // Polled through the directory's own listing, the snapshot reaches `Completed`
        // once the transition its create owes has fallen due.
        let mut status = String::new();
        for _ in 0..5 {
            let answer = ok(&router, "DescribeSnapshots", scoped.clone(), case).await;
            status = string(&answer["Snapshots"][0], "Status", case);
            if status == STATUS_COMPLETED {
                break;
            }
            tokio::time::sleep(PAST_ONE_SPAN).await;
        }
        assert_eq!(
            status, STATUS_COMPLETED,
            "case {case:?}: expected the snapshot to reach {STATUS_COMPLETED}, got {status:?}"
        );

        // Deleted, and then CONFIRMED gone through the same read the poll used: the page
        // is served and holds the snapshot no longer.
        let answer = ok(
            &router,
            "DeleteSnapshot",
            json!({ "SnapshotId": &snapshot }),
            case,
        )
        .await;
        assert_eq!(
            string(&answer, "SnapshotId", case),
            snapshot,
            "case {case:?}: expected the delete to report {snapshot}, got {answer}"
        );
        let page = reported_ids(&router, scoped.clone(), case).await;
        assert!(
            page.is_empty(),
            "case {case:?}: expected the confirming read to report no snapshot, got {page:?}"
        );

        // And the directory's whole listing agrees: only the snapshot nothing deleted is
        // left of it.
        let listing = reported_ids(&router, json!({ "DirectoryId": directory }), case).await;
        let expected: Vec<String> = if row.second_directory {
            Vec::new()
        } else {
            vec![untouched.clone()]
        };
        assert_eq!(
            listing, expected,
            "case {case:?}: expected the directory to report {expected:?}, got {listing:?}"
        );

        // A second delete of the same id is the lookup refusal, worded for the entity the
        // request named.
        let (status, parsed) = call(
            &router,
            "DeleteSnapshot",
            json!({ "SnapshotId": &snapshot }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the repeated delete to be refused, got {status} with {parsed}"
        );
        let wire_type = parsed["__type"].as_str().unwrap_or_default();
        assert!(
            wire_type.contains("EntityDoesNotExistException"),
            "case {case:?}: expected EntityDoesNotExistException, got {parsed}"
        );
        let message = stated(&parsed, case);
        assert_eq!(
            message,
            format!("Snapshot: {snapshot} does not exist"),
            "case {case:?}: unexpected diagnostic"
        );
    }
}
