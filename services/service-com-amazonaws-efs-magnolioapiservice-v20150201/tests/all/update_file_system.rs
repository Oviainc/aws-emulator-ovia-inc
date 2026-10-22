// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `UpdateFileSystem`: the throughput change it applies to the
//! stored file system, the `updating` phase it reports while the change settles, and
//! every refusal the operation models.
//!
//! The rules are asserted over a SET of inputs rather than on one call: the tables below
//! vary the starting throughput mode, the requested mode, which members the request
//! carries, the provisioned amount (both published bounds and one past the top) and the
//! form of the `FileSystemId` (bare id or ARN). The HTTP harness is the one the
//! `file_system_lifecycle` suite drives the same router through.

use serde_json::{json, Value};
use service_com_amazonaws_efs_magnolioapiservice_v20150201::state;

use crate::file_system_lifecycle::{settle, Emulator, Reply};

/// `PUT /2015-02-01/file-systems/{FileSystemId}`, the operation's modelled HTTP binding
/// (`smithy_query(query="operation",operation="UpdateFileSystem")`).
async fn update_file_system(efs: &Emulator, file_system_id: &str, body: Value) -> Reply {
    efs.send(
        "PUT",
        &format!("/2015-02-01/file-systems/{file_system_id}"),
        Some(body),
    )
    .await
}

/// Create one file system from `body` and wait out the clock-owed
/// `creating` -> `available` transition, so an update can be applied to it.
async fn settled_file_system(efs: &Emulator, body: Value) -> String {
    let created = efs.create_file_system(body.clone()).await;
    assert_eq!(
        created.status, 201,
        "case {body:?}: CreateFileSystem expected 201, got {} {:?}",
        created.status, created.body
    );
    let id = created.string("FileSystemId");
    settle().await;
    id
}

/// The stored description of one file system, read back through `DescribeFileSystems` —
/// so an assertion sees what the NEXT caller sees, not what the update echoed.
async fn stored(efs: &Emulator, file_system_id: &str) -> Value {
    let listed = efs
        .describe_file_systems(&format!("FileSystemId={file_system_id}"))
        .await;
    assert_eq!(
        listed.status, 200,
        "DescribeFileSystems({file_system_id}): expected 200, got {} {:?}",
        listed.status, listed.body
    );
    listed.get("FileSystems")[0].clone()
}

/// `ProvisionedThroughputInMibps` as a number, absent member included (`None`). Read
/// through `as_f64` so an amount the wire carries as `64` and one it carries as `64.0`
/// compare equal.
fn mibps_of(description: &Value) -> Option<f64> {
    description["ProvisionedThroughputInMibps"].as_f64()
}

// ---------------------------------------------------------------------------
// The accepted changes: each is persisted, and reported as `updating` until the
// clock owes the file system `available` again.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn each_accepted_throughput_change_is_persisted_and_settles_to_available() {
    // (case, CreateFileSystem body, UpdateFileSystem body, expected mode, expected MiBps).
    // The modes are the modelled `ThroughputMode` closure
    // (`smithy_query(query="shape",shape="ThroughputMode")` -> bursting | elastic |
    // provisioned); the amounts are the ends of the documented 1-3414 MiBps range.
    let cases: Vec<(&str, Value, Value, &str, Option<f64>)> = vec![
        (
            "bursting -> elastic",
            json!({ "CreationToken": "upd-1" }),
            json!({ "ThroughputMode": "elastic" }),
            "elastic",
            None,
        ),
        (
            "bursting -> provisioned at the lower bound",
            json!({ "CreationToken": "upd-2" }),
            json!({ "ThroughputMode": "provisioned", "ProvisionedThroughputInMibps": 1.0 }),
            "provisioned",
            Some(1.0),
        ),
        (
            "bursting -> provisioned at the upper bound",
            json!({ "CreationToken": "upd-3" }),
            json!({ "ThroughputMode": "provisioned", "ProvisionedThroughputInMibps": 3414.0 }),
            "provisioned",
            Some(3414.0),
        ),
        (
            "elastic -> bursting drops nothing else",
            json!({ "CreationToken": "upd-4", "ThroughputMode": "elastic" }),
            json!({ "ThroughputMode": "bursting" }),
            "bursting",
            None,
        ),
        (
            "provisioned -> elastic clears the amount",
            json!({
                "CreationToken": "upd-5",
                "ThroughputMode": "provisioned",
                "ProvisionedThroughputInMibps": 128.0,
            }),
            json!({ "ThroughputMode": "elastic" }),
            "elastic",
            None,
        ),
        (
            "an amount with no ThroughputMode keeps the stored mode",
            json!({
                "CreationToken": "upd-6",
                "ThroughputMode": "provisioned",
                "ProvisionedThroughputInMibps": 100.0,
            }),
            json!({ "ProvisionedThroughputInMibps": 512.0 }),
            "provisioned",
            Some(512.0),
        ),
        (
            "a decrease of the stored provisioned amount",
            json!({
                "CreationToken": "upd-7",
                "ThroughputMode": "provisioned",
                "ProvisionedThroughputInMibps": 500.0,
            }),
            json!({ "ProvisionedThroughputInMibps": 250.0 }),
            "provisioned",
            Some(250.0),
        ),
    ];

    let efs = Emulator::new();
    for (case, create_body, update_body, mode, mibps) in cases {
        let id = settled_file_system(&efs, create_body).await;
        let updated = update_file_system(&efs, &id, update_body.clone()).await;

        assert_eq!(
            updated.status, 202,
            "case {case:?}: expected HTTP 202, got {} {:?}",
            updated.status, updated.body
        );
        assert_eq!(
            updated.string("FileSystemId"),
            id,
            "case {case:?}: the description names its own file system, got {:?}",
            updated.get("FileSystemId")
        );
        assert_eq!(
            updated.string("ThroughputMode"),
            mode,
            "case {case:?}: expected ThroughputMode {mode:?}, got {:?}",
            updated.get("ThroughputMode")
        );
        assert_eq!(
            mibps_of(&updated.body),
            mibps,
            "case {case:?}: expected ProvisionedThroughputInMibps {mibps:?}, got {:?}",
            updated.get("ProvisionedThroughputInMibps")
        );

        // The change is on the record the next caller reads, whatever its lifecycle
        // phase: the update is a WRITE, not an echo.
        let before_settle = stored(&efs, &id).await;
        assert_eq!(
            before_settle["ThroughputMode"].as_str(),
            Some(mode),
            "case {case:?}: the stored file system must carry the new mode, got {before_settle:?}"
        );
        assert_eq!(
            mibps_of(&before_settle),
            mibps,
            "case {case:?}: the stored file system must carry the new amount, got {before_settle:?}"
        );

        // `updating` is clock-owed back to `available`, exactly like `creating`.
        settle().await;
        let after_settle = stored(&efs, &id).await;
        assert_eq!(
            after_settle["LifeCycleState"].as_str(),
            Some("available"),
            "case {case:?}: an applied change settles to available, got {after_settle:?}"
        );
        assert_eq!(
            after_settle["ThroughputMode"].as_str(),
            Some(mode),
            "case {case:?}: the settled file system keeps the new mode, got {after_settle:?}"
        );
        assert_eq!(
            mibps_of(&after_settle),
            mibps,
            "case {case:?}: the settled file system keeps the new amount, got {after_settle:?}"
        );
        assert_eq!(
            after_settle["CreationToken"], updated.body["CreationToken"],
            "case {case:?}: an update rewrites nothing else, got {after_settle:?}"
        );
    }
}

#[tokio::test]
async fn an_applied_change_reports_updating_and_refuses_a_second_one_until_it_settles() {
    let efs = Emulator::new();
    let id = settled_file_system(&efs, json!({ "CreationToken": "transient" })).await;

    let updated = update_file_system(
        &efs,
        &id,
        json!({ "ThroughputMode": "provisioned", "ProvisionedThroughputInMibps": 64.0 }),
    )
    .await;
    assert_eq!(
        updated.status, 202,
        "UpdateFileSystem: expected 202, got {} {:?}",
        updated.status, updated.body
    );
    assert_eq!(
        updated.string("LifeCycleState"),
        "updating",
        "UpdateFileSystem: an applied change is reported while still updating, got {:?}",
        updated.get("LifeCycleState")
    );

    // "Returned if the file system's lifecycle state is not available" — which covers a
    // file system still settling the previous change.
    let too_early = update_file_system(&efs, &id, json!({ "ProvisionedThroughputInMibps": 65.0 }))
        .await;
    assert_eq!(
        too_early.status, 409,
        "a second update while updating: expected 409, got {} {:?}",
        too_early.status, too_early.body
    );
    assert_eq!(
        too_early.error_type.as_deref(),
        Some("IncorrectFileSystemLifeCycleState"),
        "a second update while updating: unexpected error type, got {:?}",
        too_early.error_type
    );
    let stored_now = stored(&efs, &id).await;
    assert_eq!(
        mibps_of(&stored_now),
        Some(64.0),
        "a refused update must change nothing, got {stored_now:?}"
    );

    settle().await;
    let settled = stored(&efs, &id).await;
    assert_eq!(
        settled["LifeCycleState"].as_str(),
        Some("available"),
        "after the updating span the file system is available again, got {settled:?}"
    );
}

#[tokio::test]
async fn an_update_that_changes_nothing_leaves_the_file_system_available() {
    // Asking for exactly what the file system already has changes nothing, so there is
    // nothing for the service to settle: the phase stays `available`.
    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "the stored bursting mode, restated",
            json!({ "CreationToken": "noop-1" }),
            json!({ "ThroughputMode": "bursting" }),
        ),
        (
            "an empty request",
            json!({ "CreationToken": "noop-2", "ThroughputMode": "elastic" }),
            json!({}),
        ),
        (
            "the stored provisioned amount, restated",
            json!({
                "CreationToken": "noop-3",
                "ThroughputMode": "provisioned",
                "ProvisionedThroughputInMibps": 42.0,
            }),
            json!({ "ProvisionedThroughputInMibps": 42.0 }),
        ),
    ];

    let efs = Emulator::new();
    for (case, create_body, update_body) in cases {
        let id = settled_file_system(&efs, create_body.clone()).await;
        let updated = update_file_system(&efs, &id, update_body).await;
        assert_eq!(
            updated.status, 202,
            "case {case:?}: expected HTTP 202, got {} {:?}",
            updated.status, updated.body
        );
        assert_eq!(
            updated.string("LifeCycleState"),
            "available",
            "case {case:?}: a change-free update settles nothing, got {:?}",
            updated.get("LifeCycleState")
        );
        assert_eq!(
            updated.string("ThroughputMode"),
            create_body["ThroughputMode"].as_str().unwrap_or("bursting"),
            "case {case:?}: the stored mode stands, got {:?}",
            updated.get("ThroughputMode")
        );
    }
}

#[tokio::test]
async fn a_file_system_id_is_accepted_as_an_id_or_as_an_arn() {
    // `FileSystemId` takes "either a file system ID or ARN", so both forms address the
    // same stored file system. Each form gets its own file system, because the 24-hour
    // restriction below is per file system.
    let efs = Emulator::new();
    for (case, token) in [("bare id", "by-id"), ("ARN", "by-arn")] {
        let id = settled_file_system(&efs, json!({ "CreationToken": token })).await;
        let description = stored(&efs, &id).await;
        let arn = description["FileSystemArn"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            arn.ends_with(&format!("file-system/{id}")),
            "case {case:?}: expected an ARN naming {id}, got {arn:?}"
        );
        // An `@httpLabel` is not greedy, so an SDK percent-encodes the separators of an
        // ARN it puts in one; the router is what decodes them back.
        let addressed_by = if case == "ARN" {
            arn.replace(':', "%3A").replace('/', "%2F")
        } else {
            id.clone()
        };

        let updated =
            update_file_system(&efs, &addressed_by, json!({ "ThroughputMode": "elastic" })).await;
        assert_eq!(
            updated.status, 202,
            "case {case:?}: expected HTTP 202, got {} {:?}",
            updated.status, updated.body
        );
        assert_eq!(
            updated.string("FileSystemId"),
            id,
            "case {case:?}: both forms address the same file system, got {:?}",
            updated.get("FileSystemId")
        );
        assert_eq!(
            updated.string("ThroughputMode"),
            "elastic",
            "case {case:?}: expected ThroughputMode \"elastic\", got {:?}",
            updated.get("ThroughputMode")
        );
        settle().await;
    }
}

// ---------------------------------------------------------------------------
// The published 24-hour restriction on repeated throughput changes.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_mode_change_or_a_decrease_is_refused_inside_the_published_window() {
    // `TooManyRequests`: "Returned if you don't wait at least 24 hours before either
    // changing the throughput mode, or decreasing the Provisioned Throughput value." An
    // INCREASE is neither, so it is never refused for the window.
    assert_eq!(
        state::THROUGHPUT_CHANGE_COOLDOWN.as_secs(),
        24 * 60 * 60,
        "the window is the 24 hours the modelled error documents, got {:?}",
        state::THROUGHPUT_CHANGE_COOLDOWN
    );

    let efs = Emulator::new();
    let id = settled_file_system(
        &efs,
        json!({
            "CreationToken": "window",
            "ThroughputMode": "provisioned",
            "ProvisionedThroughputInMibps": 200.0,
        }),
    )
    .await;

    // The first restricted change: a decrease. Nothing has been changed before it, so
    // the window does not bind.
    let decrease = update_file_system(&efs, &id, json!({ "ProvisionedThroughputInMibps": 100.0 }))
        .await;
    assert_eq!(
        decrease.status, 202,
        "the first decrease: expected 202, got {} {:?}",
        decrease.status, decrease.body
    );
    settle().await;

    // An increase inside the window is accepted, and does not re-arm it.
    let increase = update_file_system(&efs, &id, json!({ "ProvisionedThroughputInMibps": 300.0 }))
        .await;
    assert_eq!(
        increase.status, 202,
        "an increase inside the window: expected 202, got {} {:?}",
        increase.status, increase.body
    );
    settle().await;

    // Both restricted changes are refused inside the window, and neither is applied.
    let refused: Vec<(&str, Value)> = vec![
        ("a second decrease", json!({ "ProvisionedThroughputInMibps": 150.0 })),
        ("a mode change", json!({ "ThroughputMode": "elastic" })),
        (
            "a mode change back to bursting",
            json!({ "ThroughputMode": "bursting" }),
        ),
    ];
    for (case, body) in refused {
        let reply = update_file_system(&efs, &id, body).await;
        assert_eq!(
            reply.status, 429,
            "case {case:?}: expected HTTP 429, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_type.as_deref(),
            Some("TooManyRequests"),
            "case {case:?}: unexpected error type, got {:?}",
            reply.error_type
        );
        let unchanged = stored(&efs, &id).await;
        assert_eq!(
            unchanged["ThroughputMode"].as_str(),
            Some("provisioned"),
            "case {case:?}: a refused update changes nothing, got {unchanged:?}"
        );
        assert_eq!(
            mibps_of(&unchanged),
            Some(300.0),
            "case {case:?}: a refused update changes nothing, got {unchanged:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The refusals.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_refused_update_is_the_modelled_error_and_stores_nothing() {
    let efs = Emulator::new();
    let id = settled_file_system(
        &efs,
        json!({
            "CreationToken": "refusals",
            "ThroughputMode": "provisioned",
            "ProvisionedThroughputInMibps": 100.0,
        }),
    )
    .await;
    // Still `creating`: no settle, so this one is used only by the lifecycle-state case.
    let creating = efs
        .create_file_system(json!({ "CreationToken": "refusals-creating" }))
        .await;
    let creating_id = creating.string("FileSystemId");

    // (case, FileSystemId addressed, body, expected HTTP status, expected error type).
    let cases: Vec<(&str, String, Value, u16, &str)> = vec![
        (
            "an amount with ThroughputMode bursting",
            id.clone(),
            json!({ "ThroughputMode": "bursting", "ProvisionedThroughputInMibps": 100.0 }),
            400,
            "BadRequest",
        ),
        (
            "an amount with ThroughputMode elastic",
            id.clone(),
            json!({ "ThroughputMode": "elastic", "ProvisionedThroughputInMibps": 100.0 }),
            400,
            "BadRequest",
        ),
        (
            "an amount one past the published upper bound",
            id.clone(),
            json!({ "ProvisionedThroughputInMibps": 3415.0 }),
            400,
            "ThroughputLimitExceeded",
        ),
        (
            "an amount below the modelled lower bound",
            id.clone(),
            json!({ "ProvisionedThroughputInMibps": 0.0 }),
            400,
            "ValidationException",
        ),
        (
            "a ThroughputMode outside the modelled enum",
            id.clone(),
            json!({ "ThroughputMode": "unlimited" }),
            400,
            "ValidationException",
        ),
        (
            "a file system that does not exist",
            "fs-deadbeef".to_string(),
            json!({ "ThroughputMode": "elastic" }),
            404,
            "FileSystemNotFound",
        ),
        (
            "a file system that is still creating",
            creating_id,
            json!({ "ThroughputMode": "elastic" }),
            409,
            "IncorrectFileSystemLifeCycleState",
        ),
    ];

    for (case, addressed_by, body, status, error_type) in cases {
        let refused = update_file_system(&efs, &addressed_by, body).await;
        assert_eq!(
            refused.status, status,
            "case {case:?}: expected HTTP {status}, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some(error_type),
            "case {case:?}: unexpected error type, got {:?}",
            refused.error_type
        );
    }

    // Nothing any refusal asked for reached the store.
    let unchanged = stored(&efs, &id).await;
    assert_eq!(
        unchanged["ThroughputMode"].as_str(),
        Some("provisioned"),
        "a refused update must leave the stored mode standing, got {unchanged:?}"
    );
    assert_eq!(
        mibps_of(&unchanged),
        Some(100.0),
        "a refused update must leave the stored amount standing, got {unchanged:?}"
    );
    assert_eq!(
        unchanged["LifeCycleState"].as_str(),
        Some("available"),
        "a refused update must not move the lifecycle phase, got {unchanged:?}"
    );
}

#[tokio::test]
async fn changing_to_provisioned_without_an_amount_is_refused() {
    // "If you are changing the ThroughputMode to provisioned, you must also set a value
    // for ProvisionedThroughputInMibps" — and only a file system that already stores an
    // amount can leave the member out.
    let efs = Emulator::new();
    let cases: Vec<(&str, Value, u16)> = vec![
        (
            "from bursting, with no amount",
            json!({ "CreationToken": "need-amount-1" }),
            400,
        ),
        (
            "from elastic, with no amount",
            json!({ "CreationToken": "need-amount-2", "ThroughputMode": "elastic" }),
            400,
        ),
        (
            "from provisioned, which already stores one",
            json!({
                "CreationToken": "need-amount-3",
                "ThroughputMode": "provisioned",
                "ProvisionedThroughputInMibps": 77.0,
            }),
            202,
        ),
    ];

    for (case, create_body, status) in cases {
        let id = settled_file_system(&efs, create_body).await;
        let reply = update_file_system(&efs, &id, json!({ "ThroughputMode": "provisioned" })).await;
        assert_eq!(
            reply.status, status,
            "case {case:?}: expected HTTP {status}, got {} {:?}",
            reply.status, reply.body
        );
        if status == 400 {
            assert_eq!(
                reply.error_type.as_deref(),
                Some("BadRequest"),
                "case {case:?}: unexpected error type, got {:?}",
                reply.error_type
            );
        } else {
            assert_eq!(
                mibps_of(&reply.body),
                Some(77.0),
                "case {case:?}: the stored amount stands, got {:?}",
                reply.get("ProvisionedThroughputInMibps")
            );
        }
    }
}
