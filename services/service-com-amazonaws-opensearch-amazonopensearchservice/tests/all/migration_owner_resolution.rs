// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHICH resource `GetMigration` reports as missing: a saved-object
//! migration job is addressed by its `migrationId` ALONE, and that id carries the
//! application the job imports into, so the application is what the read resolves first
//! and what its refusal names.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The harness, the application a job migrates into and
//! the per-operation request helpers come from the sibling suites; this one adds no second
//! copy of any of them.
//!
//! The tests assert RULES over sets of inputs, not one recorded call:
//!
//! * a well-formed `migrationId` whose leading segment is an `ApplicationId` this account
//!   holds no application under is refused with the modelled `ResourceNotFoundException`
//!   stating that THAT application was not found — never the whole id and never the job —
//!   over a table of ids differing in the length and the alphabet of the segment and in
//!   what the opaque half after it spells;
//! * the same holds for a job's OWN minted id once the clock has taken its application
//!   away: while the deletion is still in flight the id still reports the job, and once
//!   the removal has fallen due the very same id is refused for the application it names.

use serde_json::json;

use super::application_deletion::delete_application;
use super::control_plane_refusal_wording::message;
use super::data_source_attachments::application;
use super::domain_lifecycle::{
    error_code, string_member, Emulator, RECORDED_NOT_FOUND_STATUS, SETTLE_WAIT,
};
use super::saved_object_migrations::{get_migration, start_migration};

/// The data source a job exports saved objects out of: a search COLLECTION ARN, the shape
/// `StartMigration` validates its source against.
const SOURCE: &str = "arn:aws:aoss:us-east-1:123456789012:collection/ownerresolution";

/// Start one job for `application_id` and hand back the `migrationId` it was minted
/// under.
async fn started_job(emulator: &Emulator, application_id: &str, case: &str) -> String {
    let answer = start_migration(
        emulator,
        json!({
            "applicationId": application_id,
            "migrationOptions": {
                "source": { "datasourceArn": SOURCE },
                "workspace": { "workspaceId": "ws-owner-resolution" },
            },
        }),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200 from StartMigration, got {} with {:?}",
        answer.status, answer.body
    );
    string_member(&answer.body, "migrationId", case)
}

/// Assert that `migration_id` is refused for the application `application_id`, in the
/// words the sub-APIs of an application state that miss in — and that the refusal neither
/// names the job nor echoes the whole id back.
async fn assert_refused_for_the_application(
    emulator: &Emulator,
    migration_id: &str,
    application_id: &str,
    case: &str,
) {
    let answer = get_migration(emulator, migration_id).await;
    assert_eq!(
        (answer.status, error_code(&answer).as_str()),
        (RECORDED_NOT_FOUND_STATUS, "ResourceNotFoundException"),
        "case {case}: expected {RECORDED_NOT_FOUND_STATUS} ResourceNotFoundException, got {} \
         with {:?}",
        answer.status,
        answer.body
    );
    let expected = format!("Application {application_id} not found");
    let stated = message(&answer);
    assert_eq!(
        stated, expected,
        "case {case}: expected {expected:?}, got {stated:?}"
    );
    // The variable part of the sentence is the APPLICATION the id names, so the opaque
    // half of the id — the part the service minted and the caller quoted back — is not in
    // it at all.
    let opaque = migration_id
        .split_once('-')
        .map_or("", |(_, opaque)| opaque);
    assert!(
        !opaque.is_empty() && !stated.contains(opaque),
        "case {case}: expected the refusal not to echo the whole migrationId, got {stated:?}"
    );
}

/// RULE: a resource addressed by a subordinate id alone resolves the OWNER that id
/// carries before the subordinate record, and states that owner's own not-found sentence —
/// naming the owner id read out of the request's id — when the account holds no such
/// owner.
#[tokio::test]
async fn a_migration_id_whose_application_is_not_held_is_refused_for_that_application() {
    let emulator = Emulator::new();
    // One application the account DOES hold, so every row below is about the id the
    // request carried rather than about an empty store.
    let held = application(&emulator, "owner-resolution-held").await;
    let job = started_job(&emulator, &held, "the held application's job").await;
    assert!(
        job.starts_with(&format!("{held}-")),
        "case the held application's job: expected the minted id to name the application it \
         belongs to ({held}), got {job:?}"
    );

    // (the application segment, the opaque half after it). The segments span `Id`'s
    // `^[a-z0-9]{3,30}$` — its shortest and longest admitted value, a digits-only one and
    // a word-shaped one (source:
    // `smithy_query(query="shape",shape="ApplicationId")`) — and the opaque halves differ
    // in length, alphabet and whether they carry further hyphens, because what that half
    // spells is the service's to mint and no part of the refusal.
    let rows: Vec<(String, &str)> = vec![
        ("abc".to_string(), "1"),
        ("z".repeat(30), "0123456789abcdef"),
        ("0123456789".to_string(), "a-b-c"),
        ("nosuchapphere".to_string(), "XYZ-000"),
    ];

    for (application_id, opaque) in rows {
        assert_ne!(
            application_id, held,
            "the rows must not name the application the account holds"
        );
        let migration_id = format!("{application_id}-{opaque}");
        let case = format!("GetMigration({migration_id:?})");
        assert_refused_for_the_application(&emulator, &migration_id, &application_id, &case).await;
    }

    // Every refusal above resolved nothing away: the job of the held application still
    // reports itself.
    let read = get_migration(&emulator, &job).await;
    assert_eq!(
        read.status, 200,
        "case the held application's job after the refusals: expected 200, got {} with {:?}",
        read.status, read.body
    );
    assert_eq!(
        read.body["applicationId"],
        json!(held),
        "case the held application's job after the refusals: expected the job to report its \
         application, got {:?}",
        read.body
    );
}

/// RULE: the owner is resolved against the applications the account holds AT THE INSTANT
/// of the read, so a job's own id stops naming a job exactly when the clock takes its
/// application away — and is then refused for the application, not for the job.
#[tokio::test]
async fn a_jobs_own_id_is_refused_for_its_application_once_the_clock_removes_it() {
    let emulator = Emulator::new();
    let id = application(&emulator, "owner-resolution-removed").await;
    let job = started_job(&emulator, &id, "the job of the removed application").await;

    let read = get_migration(&emulator, &job).await;
    assert_eq!(
        read.status, 200,
        "case before the delete: expected 200 from GetMigration({job}), got {} with {:?}",
        read.status, read.body
    );

    let dropped = delete_application(&emulator, &id).await;
    assert_eq!(
        dropped.status, 200,
        "case DeleteApplication({id}): expected 200, got {} with {:?}",
        dropped.status, dropped.body
    );

    // The removal is not instant: the application is still one this account holds while
    // the delete is in flight, so the job is still addressable by its id.
    let in_flight = get_migration(&emulator, &job).await;
    assert_eq!(
        in_flight.status, 200,
        "case while the removal is in flight: expected 200 from GetMigration({job}), got {} with \
         {:?}",
        in_flight.status, in_flight.body
    );

    // Once the removal has fallen due the account holds no such application, so the SAME
    // id names nothing to look a job up in.
    tokio::time::sleep(SETTLE_WAIT).await;
    assert_refused_for_the_application(
        &emulator,
        &job,
        &id,
        &format!("GetMigration({job:?}) after the removal fell due"),
    )
    .await;
}
