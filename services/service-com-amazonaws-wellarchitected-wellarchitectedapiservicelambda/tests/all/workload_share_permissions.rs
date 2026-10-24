// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `UpdateWorkloadShare`: changing the permission an existing
//! share of a workload grants.
//!
//! The rules pinned here, each over a TABLE of inputs rather than one recorded request:
//!
//! 1. `UpdateWorkloadShare` replaces the permission the named share grants with the
//!    `PermissionType` the request asks for and persists it: the answer's `WorkloadShare`
//!    reports the requested permission and `ListWorkloadShares` reads it back. The rows
//!    span every (held permission, requested permission) pair of the modelled enum,
//!    including the pair that asks for the permission the share already grants.
//! 2. The permission is the WHOLE of what the operation changes: the share's id, the
//!    principal it was offered to, the principal that shared it and its share status are
//!    untouched, whatever status the share is in — the rows span every `ShareStatus`
//!    member. `WorkloadName` is reported from the workload, so a workload renamed between
//!    the share's creation and the update is reported under its current name.
//! 3. The share is resolved by the `(WorkloadId, ShareId)` pair the path carries: a
//!    workload id that resolves nothing, a share id the workload does not hold, and a
//!    share id that belongs to ANOTHER workload are each `404
//!    ResourceNotFoundException` and change no stored share.
//! 4. A `PermissionType` the enum does not publish is refused `400 ValidationException`
//!    and leaves the share's permission as it was.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do
//! in production. The HTTP client and the request builders are the shared
//! `super::workload_harness`.

use serde_json::{json, Value};

use super::workload_harness::{created_id, minimal_create, Client};

/// A well-formed `WorkloadId` (32 lowercase hex) that no workload carries.
const ABSENT_WORKLOAD_ID: &str = "0123456789abcdef0123456789abcdef";

/// A well-formed `ShareId` (`^[0-9a-f]{32}$`) that no share carries.
const ABSENT_SHARE_ID: &str = "ffffffffffffffffffffffffffffffff";

/// The `PermissionType` members the model publishes —
/// `smithy_query(query="shape", shape="PermissionType")` lists exactly these two.
const PERMISSION_TYPES: [&str; 2] = ["CONTRIBUTOR", "READONLY"];

/// The `ShareStatus` members the model publishes —
/// `smithy_query(query="shape", shape="ShareStatus")` lists exactly these eight.
const SHARE_STATUSES: [&str; 8] = [
    "ACCEPTED",
    "ASSOCIATED",
    "ASSOCIATING",
    "EXPIRED",
    "FAILED",
    "PENDING",
    "REJECTED",
    "REVOKED",
];

/// Principals a workload can be shared WITH: "The Amazon Web Services account ID,
/// organization ID, or organizational unit (OU) ID with which the workload … is shared"
/// (`SharedWith` doc, `@length(12, 2048)` and no pattern), so the rows span all three
/// kinds rather than only an account id.
const PRINCIPALS: [&str; 4] = [
    "111122223333",
    "444455556666",
    "o-abcd1234efgh",
    "ou-abcd-1a2b3c4d",
];

fn string_member<'a>(body: &'a Value, path: &[&str]) -> &'a str {
    let mut cursor = body;
    for step in path {
        cursor = match cursor.get(step) {
            Some(value) => value,
            None => return "",
        };
    }
    cursor.as_str().unwrap_or_default()
}

/// Create a workload and one share of it over HTTP, and answer
/// `(workload_id, workload_name, share_id)`.
async fn workload_with_share(
    client: &Client,
    case: &str,
    shared_with: &str,
    permission_type: &str,
) -> (String, String, String) {
    let workload_name = format!("shared-{case}");
    let created = client
        .create(minimal_create(&workload_name, &format!("create-{case}")))
        .await;
    let workload_id = created_id(&created, case);
    let share = client
        .create_share(
            &workload_id,
            json!({
                "SharedWith": shared_with,
                "PermissionType": permission_type,
                "ClientRequestToken": format!("share-{case}"),
            }),
        )
        .await;
    assert_eq!(
        share.status, 200,
        "case {case:?}: expected CreateWorkloadShare 200, got {share:?}"
    );
    let share_id = string_member(&share.body, &["ShareId"]).to_string();
    assert!(
        share_id.len() == 32 && share_id.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')),
        "case {case:?}: ShareId must match ^[0-9a-f]{{32}}$, got {share_id:?}"
    );
    (workload_id, workload_name, share_id)
}

/// The one summary `ListWorkloadShares` reports for `share_id`, or `None`.
async fn listed_share(client: &Client, workload_id: &str, share_id: &str, case: &str) -> Value {
    let answer = client.list_shares(workload_id, "").await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected ListWorkloadShares 200, got {answer:?}"
    );
    answer
        .body
        .get("WorkloadShareSummaries")
        .and_then(Value::as_array)
        .and_then(|summaries| {
            summaries
                .iter()
                .find(|summary| string_member(summary, &["ShareId"]) == share_id)
                .cloned()
        })
        .unwrap_or_else(|| {
            panic!("case {case:?}: ListWorkloadShares must still report share {share_id:?}, got {answer:?}")
        })
}

/// Rule 1 (and the idempotent row of it): every (held, requested) permission pair is a
/// 200 whose `WorkloadShare` grants the requested permission, and `ListWorkloadShares`
/// reads that permission back.
#[tokio::test]
async fn updating_a_share_grants_the_requested_permission_and_persists_it() {
    let client = Client::new();

    for (index, held) in PERMISSION_TYPES.iter().enumerate() {
        for (offset, requested) in PERMISSION_TYPES.iter().enumerate() {
            let case = format!("{held}-to-{requested}");
            let shared_with = PRINCIPALS[(index * PERMISSION_TYPES.len() + offset) % PRINCIPALS.len()];
            let (workload_id, workload_name, share_id) =
                workload_with_share(&client, &case, shared_with, held).await;

            let answer = client
                .update_share(
                    &workload_id,
                    &share_id,
                    json!({ "PermissionType": requested }),
                )
                .await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected UpdateWorkloadShare 200, got {answer:?}"
            );
            assert_eq!(
                string_member(&answer.body, &["WorkloadId"]),
                workload_id,
                "case {case:?}: expected the updated share's WorkloadId {workload_id:?}, got {answer:?}"
            );
            let share = answer
                .body
                .get("WorkloadShare")
                .cloned()
                .unwrap_or(Value::Null);
            assert_eq!(
                string_member(&share, &["PermissionType"]),
                *requested,
                "case {case:?}: expected PermissionType {requested:?}, got {share:?}"
            );
            assert_eq!(
                string_member(&share, &["ShareId"]),
                share_id,
                "case {case:?}: expected ShareId {share_id:?}, got {share:?}"
            );
            assert_eq!(
                string_member(&share, &["SharedWith"]),
                shared_with,
                "case {case:?}: expected SharedWith {shared_with:?}, got {share:?}"
            );
            assert_eq!(
                string_member(&share, &["SharedBy"]),
                engine::emulator_account_id(),
                "case {case:?}: expected SharedBy to be the sharing account, got {share:?}"
            );
            assert_eq!(
                string_member(&share, &["WorkloadId"]),
                workload_id,
                "case {case:?}: expected the share's WorkloadId {workload_id:?}, got {share:?}"
            );
            assert_eq!(
                string_member(&share, &["WorkloadName"]),
                workload_name,
                "case {case:?}: expected WorkloadName {workload_name:?}, got {share:?}"
            );

            let listed = listed_share(&client, &workload_id, &share_id, &case).await;
            assert_eq!(
                string_member(&listed, &["PermissionType"]),
                *requested,
                "case {case:?}: ListWorkloadShares must read the updated permission \
                 {requested:?} back, got {listed:?}"
            );
            assert_eq!(
                string_member(&listed, &["SharedWith"]),
                shared_with,
                "case {case:?}: the stored share must keep its principal, got {listed:?}"
            );
        }
    }
}

/// Rule 2: whatever `ShareStatus` the share is in, the update changes only the
/// permission — the status survives in both the answer and the stored share. The share
/// is seeded through the shared state because nothing in this emulator can drive a share
/// out of `PENDING` yet (`UpdateShareInvitation` is still a stub with its own issue);
/// the update under test still crosses the HTTP boundary.
#[tokio::test]
async fn updating_a_share_leaves_its_status_and_principals_alone() {
    let client = Client::new();

    for (index, status) in SHARE_STATUSES.iter().enumerate() {
        let case = format!("status-{status}");
        let workload_name = format!("status-workload-{status}");
        let created = client
            .create(minimal_create(&workload_name, &format!("create-{case}")))
            .await;
        let workload_id = created_id(&created, &case);
        let shared_with = PRINCIPALS[index % PRINCIPALS.len()];
        let held = PERMISSION_TYPES[index % PERMISSION_TYPES.len()];
        let requested = PERMISSION_TYPES[(index + 1) % PERMISSION_TYPES.len()];
        let (share_id, _invitation_id) =
            client.record_workload_share(&workload_id, shared_with, held, status);

        let answer = client
            .update_share(
                &workload_id,
                &share_id,
                json!({ "PermissionType": requested }),
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected UpdateWorkloadShare 200, got {answer:?}"
        );
        let share = answer
            .body
            .get("WorkloadShare")
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            string_member(&share, &["Status"]),
            *status,
            "case {case:?}: expected the share to keep status {status:?}, got {share:?}"
        );
        assert_eq!(
            string_member(&share, &["PermissionType"]),
            requested,
            "case {case:?}: expected PermissionType {requested:?}, got {share:?}"
        );
        assert_eq!(
            string_member(&share, &["SharedWith"]),
            shared_with,
            "case {case:?}: expected SharedWith {shared_with:?}, got {share:?}"
        );

        let listed = listed_share(&client, &workload_id, &share_id, &case).await;
        assert_eq!(
            string_member(&listed, &["Status"]),
            *status,
            "case {case:?}: the stored share must keep status {status:?}, got {listed:?}"
        );
        assert_eq!(
            string_member(&listed, &["PermissionType"]),
            requested,
            "case {case:?}: the stored share must grant {requested:?}, got {listed:?}"
        );
    }
}

/// Rule 2's `WorkloadName`: the share reports the name the workload carries NOW, so a
/// workload renamed after the share was created is reported under its new name.
#[tokio::test]
async fn the_updated_share_reports_the_workloads_current_name() {
    let client = Client::new();
    let case = "renamed-workload";
    let (workload_id, original_name, share_id) =
        workload_with_share(&client, case, PRINCIPALS[0], PERMISSION_TYPES[0]).await;

    let renamed = "renamed-after-sharing";
    let rename = client
        .update(&workload_id, json!({ "WorkloadName": renamed }))
        .await;
    assert_eq!(
        rename.status, 200,
        "case {case:?}: expected UpdateWorkload 200, got {rename:?}"
    );
    assert_ne!(
        original_name, renamed,
        "case {case:?}: the rename must actually change the name"
    );

    let answer = client
        .update_share(
            &workload_id,
            &share_id,
            json!({ "PermissionType": PERMISSION_TYPES[1] }),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected UpdateWorkloadShare 200, got {answer:?}"
    );
    let share = answer
        .body
        .get("WorkloadShare")
        .cloned()
        .unwrap_or(Value::Null);
    assert_eq!(
        string_member(&share, &["WorkloadName"]),
        renamed,
        "case {case:?}: expected WorkloadName {renamed:?}, got {share:?}"
    );
}

/// Rule 3: the pair in the path is what resolves the share, so an unresolvable workload,
/// an unheld share id and a share id of another workload are each a 404 that changes no
/// stored share.
#[tokio::test]
async fn an_unresolvable_workload_or_share_is_not_found_and_changes_nothing() {
    let client = Client::new();
    let (first_id, _first_name, first_share) =
        workload_with_share(&client, "first", PRINCIPALS[0], "CONTRIBUTOR").await;
    let (second_id, _second_name, second_share) =
        workload_with_share(&client, "second", PRINCIPALS[1], "READONLY").await;

    // (case, workload id, share id) — each row names a pair no workload holds.
    let rows: [(&str, &str, &str); 4] = [
        ("absent-workload", ABSENT_WORKLOAD_ID, first_share.as_str()),
        ("absent-share", first_id.as_str(), ABSENT_SHARE_ID),
        // The share id is real, but it belongs to the OTHER workload.
        ("share-of-another-workload", second_id.as_str(), first_share.as_str()),
        ("neither", ABSENT_WORKLOAD_ID, ABSENT_SHARE_ID),
    ];

    for (case, workload_id, share_id) in rows {
        let answer = client
            .update_share(workload_id, share_id, json!({ "PermissionType": "READONLY" }))
            .await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected UpdateWorkloadShare 404, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {answer:?}"
        );
    }

    let first_listed = listed_share(&client, &first_id, &first_share, "first-unchanged").await;
    assert_eq!(
        string_member(&first_listed, &["PermissionType"]),
        "CONTRIBUTOR",
        "a refused update must leave the first workload's share as it was, got {first_listed:?}"
    );
    let second_listed = listed_share(&client, &second_id, &second_share, "second-unchanged").await;
    assert_eq!(
        string_member(&second_listed, &["PermissionType"]),
        "READONLY",
        "a refused update must leave the second workload's share as it was, got {second_listed:?}"
    );
}

/// Rule 4: a `PermissionType` outside the modelled enum (or absent) is a 400
/// `ValidationException` and the share keeps the permission it had.
#[tokio::test]
async fn a_permission_outside_the_modelled_enum_is_refused() {
    let client = Client::new();
    let (workload_id, _name, share_id) =
        workload_with_share(&client, "bad-permission", PRINCIPALS[2], "CONTRIBUTOR").await;

    // (case, request body) — values just outside the published value set
    // ["READONLY", "CONTRIBUTOR"], plus the request that omits the required member.
    let rows: [(&str, Value); 6] = [
        ("empty", json!({ "PermissionType": "" })),
        ("lowercase", json!({ "PermissionType": "readonly" })),
        ("trailing-space", json!({ "PermissionType": "CONTRIBUTOR " })),
        ("unmodelled-member", json!({ "PermissionType": "ADMIN" })),
        ("owner", json!({ "PermissionType": "OWNER" })),
        ("absent", json!({})),
    ];

    for (case, body) in rows {
        let answer = client.update_share(&workload_id, &share_id, body).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected UpdateWorkloadShare 400, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ValidationException",
            "case {case:?}: expected ValidationException, got {answer:?}"
        );
        let listed = listed_share(&client, &workload_id, &share_id, case).await;
        assert_eq!(
            string_member(&listed, &["PermissionType"]),
            "CONTRIBUTOR",
            "case {case:?}: a refused update must leave the permission as it was, got {listed:?}"
        );
    }
}

/// `ListWorkloadShares` is a report, so reading the share back never changes it: the
/// same read repeated after an update answers the same thing.
#[tokio::test]
async fn reading_the_updated_share_back_twice_answers_the_same() {
    let client = Client::new();
    let (workload_id, _name, share_id) =
        workload_with_share(&client, "stable-read", PRINCIPALS[3], "READONLY").await;
    let updated = client
        .update_share(
            &workload_id,
            &share_id,
            json!({ "PermissionType": "CONTRIBUTOR" }),
        )
        .await;
    assert_eq!(
        updated.status, 200,
        "expected UpdateWorkloadShare 200, got {updated:?}"
    );

    let first = listed_share(&client, &workload_id, &share_id, "first-read").await;
    let second = listed_share(&client, &workload_id, &share_id, "second-read").await;
    assert_eq!(
        first, second,
        "case \"stable-read\": repeating the read must answer the same, got {first:?} then {second:?}"
    );
    assert_eq!(
        string_member(&second, &["PermissionType"]),
        "CONTRIBUTOR",
        "case \"stable-read\": expected the updated permission, got {second:?}"
    );
}
