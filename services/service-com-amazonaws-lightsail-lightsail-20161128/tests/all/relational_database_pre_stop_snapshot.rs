// SPDX-License-Identifier: CC0-1.0

//! The pre-stop snapshot a `StopRelationalDatabase` call takes — driven over the
//! wire.
//!
//! Every case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rules under test, asserted over the same SET of databases
//! `relational_database_start_stop.rs` stops and starts — both engines this
//! service publishes blueprints for, several engine versions, standard and
//! high-availability bundles of different sizes, in four Regions:
//!
//! * `relationalDatabaseSnapshotName` is "the name of your new database snapshot
//!   to be created before stopping your database", so a stop that names one
//!   stores the SAME point-in-time copy of the still-serving database that a
//!   `CreateRelationalDatabaseSnapshot` call of that database stores, with an
//!   identity of its own, and that copy is not restorable the instant the call
//!   returns: the clock owes it `available`; and
//! * because it is that copy, it carries the master credentials a restore needs:
//!   `CreateRelationalDatabaseFromSnapshot` restores from it a serving database
//!   that is reached with the captured credentials and runs the captured engine,
//!   while the database the copy was taken of stays stopped.
//!
//! The harness, the database table and the waits are the ones the sibling suites
//! already expose, so nothing here is a second copy of them.

use serde_json::{json, Value};

use crate::instance_lifecycle::Emulator;
use crate::relational_database_master_user_password::read_password;
use crate::relational_database_restore_from_snapshot::{read_snapshot, wait_for_snapshot};
use crate::relational_database_start_stop::{
    database, wait_for_state, with_available_database, without, Row, ROWS,
};

/// The members a stored snapshot holds of its OWN identity rather than of the
/// database it was taken of: two copies of one database taken in the same state
/// differ on exactly these.
const SNAPSHOT_IDENTITY_MEMBERS: &[&str] = &["name", "arn", "supportCode", "createdAt"];

/// `snapshot` without the members that are its own identity, so two copies can be
/// compared on everything they say about the database they were taken of.
fn without_identity(snapshot: &Value) -> Value {
    SNAPSHOT_IDENTITY_MEMBERS
        .iter()
        .fold(snapshot.clone(), |record, member| without(&record, member))
}

/// The current master user password of the database `name`, read through the one
/// call that answers the secret.
///
/// The secret is compared, never reported: a case that fails says which database
/// disagreed, not what either password is.
async fn master_password(emulator: &Emulator, name: &str) -> String {
    let answered = read_password(emulator, name, None).await;
    answered["masterUserPassword"]
        .as_str()
        .unwrap_or_else(|| panic!("database {name}: the answer carries no masterUserPassword"))
        .to_string()
}

/// Stop the database `name`, taking the pre-stop snapshot `snapshot_name`.
async fn stop_with_snapshot(emulator: &Emulator, name: &str, snapshot_name: &str) {
    emulator
        .ok(
            "StopRelationalDatabase",
            json!({
                "relationalDatabaseName": name,
                "relationalDatabaseSnapshotName": snapshot_name,
            }),
        )
        .await;
}

/// RULE: a stop's pre-stop snapshot is the SAME point-in-time copy of the
/// still-serving database that a `CreateRelationalDatabaseSnapshot` call of that
/// database stores — the two records differ only on the identity each copy holds
/// of its own — and it is not restorable the instant the call returns, because
/// the clock owes it `available`.
#[tokio::test]
async fn a_pre_stop_snapshot_is_the_copy_a_create_snapshot_call_stores() {
    for row in ROWS {
        let Row { case, .. } = row;
        let name = "copied-db";
        let emulator = with_available_database(row, name).await;

        // The control copy, taken through the operation whose only job is to take
        // one, while the database is still serving.
        emulator
            .ok(
                "CreateRelationalDatabaseSnapshot",
                json!({
                    "relationalDatabaseName": name,
                    "relationalDatabaseSnapshotName": "control-copy",
                }),
            )
            .await;
        let control = wait_for_snapshot(&emulator, "control-copy", "available").await;

        // The pre-stop copy, taken by a stop of the same database in the same
        // state.
        stop_with_snapshot(&emulator, name, "pre-stop-copy").await;

        // A copy is not restorable the instant the call returns: the clock owes
        // it `available`, so a caller reading straight after the stop sees the
        // transient state or the settled one and never a third.
        let fresh = read_snapshot(&emulator, "pre-stop-copy").await;
        assert!(
            matches!(fresh["state"].as_str(), Some("creating") | Some("available")),
            "case {case}: expected the pre-stop copy creating or already settled, got {fresh:?}"
        );
        let pre_stop = wait_for_snapshot(&emulator, "pre-stop-copy", "available").await;

        // Everything either copy says about the database it was taken of is the
        // same, because both were taken of the same record in the same state.
        assert_eq!(
            without_identity(&pre_stop),
            without_identity(&control),
            "case {case}: expected the pre-stop copy to describe its source exactly as the \
             CreateRelationalDatabaseSnapshot copy does, got {pre_stop:?} against {control:?}"
        );
        // The identity is its own: a copy is a resource of its own, so it carries
        // the name the request gave it and an ARN nothing else holds.
        assert_eq!(
            pre_stop["name"].as_str(),
            Some("pre-stop-copy"),
            "case {case}: expected the copy to carry the name the request gave it, got \
             {pre_stop:?}"
        );
        assert_ne!(
            pre_stop["arn"], control["arn"],
            "case {case}: expected each copy to carry an ARN of its own, got {:?} for both",
            pre_stop["arn"]
        );

        // The stop the copy was taken before still happened.
        wait_for_state(&emulator, name, "stopped").await;
    }
}

/// RULE: the pre-stop copy captures the master credentials a restore needs, so
/// `CreateRelationalDatabaseFromSnapshot` restores from it a serving database
/// carrying the source's engine, version, served database name and master
/// credentials with an identity of its own — and the database the copy was taken
/// of stays stopped, because the restore acts on the copy and not on it.
#[tokio::test]
async fn a_pre_stop_snapshot_restores_a_database_while_its_source_stays_stopped() {
    for row in ROWS {
        let Row {
            case,
            availability_zone,
            ..
        } = row;
        let name = "restorable-db";
        let restored_name = "restored-from-pre-stop";
        let emulator = with_available_database(row, name).await;
        let serving = database(&emulator, name).await;

        stop_with_snapshot(&emulator, name, "pre-stop-copy").await;
        let pre_stop = wait_for_snapshot(&emulator, "pre-stop-copy", "available").await;
        wait_for_state(&emulator, name, "stopped").await;

        // A snapshot is restored in the Region it was taken in, which is where the
        // zone the restore names places the restored database.
        emulator
            .ok(
                "CreateRelationalDatabaseFromSnapshot",
                json!({
                    "relationalDatabaseName": restored_name,
                    "relationalDatabaseSnapshotName": "pre-stop-copy",
                    "availabilityZone": availability_zone,
                }),
            )
            .await;
        let restored = wait_for_state(&emulator, restored_name, "available").await;

        // What the copy captured is what the restored database serves, each
        // member required to be reported at all so the comparison cannot pass on
        // two absent values.
        for member in [
            "engine",
            "engineVersion",
            "masterUsername",
            "masterDatabaseName",
            "relationalDatabaseBlueprintId",
            "relationalDatabaseBundleId",
        ] {
            assert!(
                serving[member].is_string(),
                "case {case}: expected the serving database to report {member}, got {serving:?}"
            );
            assert_eq!(
                restored[member], serving[member],
                "case {case}: expected the restored database's {member} to be the captured {:?}, \
                 got {:?}",
                serving[member], restored[member]
            );
        }
        assert!(
            pre_stop["sizeInGb"].is_number(),
            "case {case}: expected the copy to report a disk size, got {pre_stop:?}"
        );
        assert_eq!(
            restored["hardware"]["diskSizeInGb"], pre_stop["sizeInGb"],
            "case {case}: expected the restored disk to be the copy's {:?}, got {:?}",
            pre_stop["sizeInGb"], restored["hardware"]["diskSizeInGb"]
        );
        let captured = master_password(&emulator, name).await;
        let restored_password = master_password(&emulator, restored_name).await;
        assert!(
            restored_password == captured,
            "case {case}: expected the restored database to be reached with the master password \
             the copy captured, got a different one"
        );

        // Its identity is newly minted rather than the source's, each member
        // required to be reported at all.
        for member in ["name", "arn", "supportCode"] {
            assert!(
                restored[member].is_string(),
                "case {case}: expected the restored database to report {member}, got {restored:?}"
            );
            assert_ne!(
                restored[member], serving[member],
                "case {case}: expected the restored database's {member} to be its own, got {:?} \
                 for both",
                restored[member]
            );
        }
        assert!(
            restored["masterEndpoint"]["address"].is_string(),
            "case {case}: expected the restored database to announce an endpoint, got {restored:?}"
        );
        assert_ne!(
            restored["masterEndpoint"]["address"], serving["masterEndpoint"]["address"],
            "case {case}: expected the restored database to announce an endpoint of its own, got \
             {:?} for both",
            restored["masterEndpoint"]["address"]
        );

        // The restore acted on the copy, so the database the copy was taken of is
        // still stopped: nothing started it.
        let source = database(&emulator, name).await;
        assert_eq!(
            source["state"].as_str(),
            Some("stopped"),
            "case {case}: expected the stopped source to stay stopped across the restore, got \
             {source:?}"
        );
    }
}
