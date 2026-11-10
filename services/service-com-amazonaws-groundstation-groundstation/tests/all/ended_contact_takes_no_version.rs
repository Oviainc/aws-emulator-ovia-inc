// SPDX-License-Identifier: CC0-1.0

//! What becomes of a version change still in flight when the contact it was appended to
//! ENDS before that change was applied.
//!
//! The RULE under test: a clock-owed move belongs to a resource that still has somewhere
//! to go, so once nothing of a contact is still to be flown — its reservation has been
//! released (the cancel is accepted, whichever side of the pass it arrived on) or it holds
//! a status it can never leave — the service owes it no further move of its own, and the
//! `UPDATING` -> `ACTIVE` move of a version an update left in flight is WITHDRAWN rather
//! than materialised later. Every read taken after the contact ended therefore reports the
//! versions, their stamps and the contact's `lastUpdated` exactly as the end left them.
//! While a pass can still be flown, by contrast, a change already in flight is still
//! applied: the rule is about a contact with no pass left, not about one whose pass has
//! begun.
//!
//! The rows of the table differ along what the rule generalises over: HOW the contact
//! ended (a cancel before the pass, a cancel of a pass under way from each status that
//! stops, and each of the statuses a contact can never leave —
//! `state::TERMINAL_CONTACT_STATUSES`), how many version changes were left in flight (one,
//! or two chained), and which members the update named (`satelliteArn` alone,
//! `trackingOverrides` alone, both). The rows where the pass is still to be flown are the
//! control: the same in-flight change, and it does land.
//!
//! Every request goes through the real front door — the router, deserialization,
//! constraint validation, the handler and serialization — on the harness
//! `scheduling_loop.rs` stands up, and the three readers of a version
//! (`DescribeContact`, `ListContactVersions`, `DescribeContactVersion`) are cross-checked
//! so the answer cannot differ per operation.

use std::time::Instant;

use serde_json::{json, Value};
use service_com_amazonaws_groundstation_groundstation::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::contact_cancellation::put_in_status;
use crate::contact_satellite_reference_integrity::onboard_satellite_arns;
use crate::minute_usage::reserve_pass;
use crate::scheduling_loop::{
    create_mission_profile, harness_with_engine, send, Reply, AFTER_SETTLE,
};

/// The first window this suite reserves: far enough ahead that no reservation is refused
/// for having elapsed, and that no row's pass is carried along by the clock during the run.
const PASS_START: i64 = 1_960_000_000;

/// How long each reserved pass runs.
const PASS_SECONDS: i64 = 600;

/// What ends a row's contact — or does not — after its version change is in flight.
#[derive(Debug, Clone, Copy)]
enum Ending {
    /// Nothing ends it: the reservation stands with its pass still ahead, so the change
    /// in flight has a pass to take force for.
    None,
    /// The contact is put into a status its pass carries it to and left there: the pass is
    /// being flown, so the change in flight still lands.
    UnderWay(&'static str),
    /// `CancelContact` releases the reservation. The contact is first put into `from`,
    /// which is which side of the pass the cancel arrives on.
    Cancel { from: &'static str },
    /// A status the contact can never leave, written into the store.
    Terminal(&'static str),
}

/// One row: what ends the contact, and whether the version change in flight is still
/// applied afterwards.
struct Row {
    case: String,
    ending: Ending,
    /// Whether the `UPDATING` -> `ACTIVE` move the update left in flight is still owed.
    applied: bool,
    /// How many version changes the row leaves in flight (so `versions` 2.. are pending).
    pending: usize,
}

/// A row's contact once it has been set up: its id, and what the readers answered right
/// after the ending — the state every assertion below is taken against.
struct Prepared {
    row: Row,
    contact_id: String,
    /// `DescribeContact`'s whole answer, taken right after the ending.
    before: Reply,
}

/// The three shapes an `UpdateContact` body takes — the configuration an update can carry
/// is `satelliteArn` and `trackingOverrides` (`smithy_query(query="operation",
/// operation="UpdateContact")`), so a request names one, the other, or both.
fn update_body(form: usize, satellite: &str) -> Value {
    match form % 3 {
        0 => json!({"satelliteArn": satellite}),
        1 => json!({"trackingOverrides": {}}),
        _ => json!({"satelliteArn": satellite, "trackingOverrides": {}}),
    }
}

/// `UpdateContact` as the model routes it: `POST /contact/{contactId}/versions`.
async fn update_contact(router: &MultiServiceRouter, contact_id: &str, body: Value) -> Reply {
    send(
        router,
        "POST",
        &format!("/contact/{contact_id}/versions"),
        Some(body),
    )
    .await
}

/// `DescribeContact`, asserting it was answered.
async fn describe_contact(router: &MultiServiceRouter, case: &str, contact_id: &str) -> Reply {
    let described = send(router, "GET", &format!("/contact/{contact_id}"), None).await;
    assert_eq!(
        described.status, 200,
        "case {case}: DescribeContact expected 200, got {} {}",
        described.status, described.body
    );
    described
}

/// `ListContactVersions`' whole history for `contact_id`, oldest first.
async fn version_history(
    router: &MultiServiceRouter,
    case: &str,
    contact_id: &str,
) -> Vec<Value> {
    let listed = send(
        router,
        "GET",
        &format!("/contact/{contact_id}/versions"),
        None,
    )
    .await;
    assert_eq!(
        listed.status, 200,
        "case {case}: ListContactVersions expected 200, got {} {}",
        listed.status, listed.body
    );
    listed
        .field("contactVersionsList")
        .as_array()
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: contactVersionsList must be a list, got {}",
                listed.body
            )
        })
}

/// `DescribeContactVersion`'s `version` record for one `versionId` of `contact_id`.
async fn described_version(
    router: &MultiServiceRouter,
    case: &str,
    contact_id: &str,
    version_id: i64,
) -> Value {
    let described = send(
        router,
        "GET",
        &format!("/contact/{contact_id}/versions/{version_id}"),
        None,
    )
    .await;
    assert_eq!(
        described.status, 200,
        "case {case}: DescribeContactVersion expected 200 for version {version_id}, got {} {}",
        described.status, described.body
    );
    described.field("version").clone()
}

/// The members of a version record that say where the version STANDS, with its
/// `lastUpdated` left out: a version's last change is its supersession, else its
/// activation, else its creation, so `lastUpdated` is settled by the three instants
/// above and comparing it again adds nothing.
fn version_standing(version: &Value) -> Value {
    json!({
        "versionId": version.get("versionId").cloned(),
        "status": version.get("status").cloned(),
        "created": version.get("created").cloned(),
        "activated": version.get("activated").cloned(),
        "superseded": version.get("superseded").cloned(),
    })
}

/// The standing of every record of a history.
fn history_standing(history: &[Value]) -> Vec<Value> {
    history.iter().map(version_standing).collect()
}

/// The `lastUpdated` the contact's NEWEST version reports as it reaches the wire — the
/// version each row's in-flight change appended (`ContactVersion$lastUpdated` —
/// `smithy_query(query="operation", operation="DescribeContact")`, whose output carries
/// the contact's `version`), so it stands still unless that version itself moves.
fn reported_last_updated(described: &Reply) -> Value {
    described
        .field("version")
        .get("lastUpdated")
        .cloned()
        .unwrap_or(Value::Null)
}

/// Every row of the table: the endings that leave the contact with no pass to fly, and the
/// ones that do not.
fn rows() -> Vec<Row> {
    let mut rows = vec![
        Row {
            case: "nothing ends the contact".to_string(),
            ending: Ending::None,
            applied: true,
            pending: 1,
        },
        Row {
            case: "a reservation whose pass is still ahead, two changes in flight".to_string(),
            ending: Ending::None,
            applied: true,
            pending: 2,
        },
    ];
    // The pass being flown is not an ending: the antenna is working the contact, so a
    // change already in flight still takes force. One row per status of the pass itself.
    for status in state::STATUSES_CANCELLABLE_DURING_PASS {
        rows.push(Row {
            case: format!("the pass is under way in {status}"),
            ending: Ending::UnderWay(status),
            applied: true,
            pending: 1,
        });
    }
    // A cancel from each side of the pass: before it the contact is released outright,
    // during it the release is carried out as a stop.
    for status in state::STATUSES_CANCELLABLE_BEFORE_PASS
        .iter()
        .chain(state::STATUSES_CANCELLABLE_DURING_PASS)
    {
        rows.push(Row {
            case: format!("a cancel arriving in {status}"),
            ending: Ending::Cancel { from: status },
            applied: false,
            pending: 1,
        });
    }
    // And a cancel with two changes chained into flight, so the withdrawal is not about
    // there being exactly one.
    rows.push(Row {
        case: "a cancel with two changes in flight".to_string(),
        ending: Ending::Cancel {
            from: state::CONTACT_SCHEDULED,
        },
        applied: false,
        pending: 2,
    });
    // Every status a contact can never leave, whether it was released, flown to the end or
    // failed: nothing of it is still to be flown either.
    for status in state::TERMINAL_CONTACT_STATUSES {
        rows.push(Row {
            case: format!("a contact that can never leave {status}"),
            ending: Ending::Terminal(status),
            applied: false,
            pending: 1,
        });
    }
    rows
}

#[tokio::test]
async fn a_contact_with_no_pass_left_takes_none_of_the_version_changes_still_in_flight() {
    let (router, engine) = harness_with_engine();
    let profile_arn = create_mission_profile(&router, "ended contacts", 60, Some(60), Some(30)).await;
    // A contact is flown against a satellite of THIS account, so the ARNs the updates name
    // are on-boarded first; two of them, so a chained pair of updates names a different one
    // each time.
    let satellites = onboard_satellite_arns(&engine, 2);

    // Every row is set up back to back and the span waited out ONCE: the move each update
    // left in flight is owed from its own call, so a single wait outlasts them all.
    let started = Instant::now();
    let mut prepared: Vec<Prepared> = Vec::new();
    for (slot, row) in rows().into_iter().enumerate() {
        // A window of its own per row: one antenna serves one pass at a time, so two rows
        // cannot hold the same window at one ground station.
        let row_start = PASS_START + i64::try_from(slot).expect("a row index fits in i64") * 86_400;
        let contact_id =
            reserve_pass(&router, &profile_arn, &row.case, row_start, PASS_SECONDS).await;

        // The changes this row leaves in flight, appended while the pass is still ahead of
        // the contact — the only statuses an update can be STARTED from.
        for change in 0..row.pending {
            let expected_version = state::FIRST_VERSION_ID
                + 1
                + i32::try_from(change).expect("a change index fits in i32");
            let body = update_body(slot + change, &satellites[change % satellites.len()]);
            let updated = update_contact(&router, &contact_id, body.clone()).await;
            assert_eq!(
                updated.status, 200,
                "case {}: UpdateContact {body} expected 200, got {} {}",
                row.case, updated.status, updated.body
            );
            assert_eq!(
                updated.field("versionId"),
                &json!(expected_version),
                "case {}: an update appends version {expected_version}, got {}",
                row.case,
                updated.field("versionId")
            );
        }

        // Then the ending, which is what the row is about.
        match row.ending {
            Ending::None => {}
            Ending::UnderWay(status) | Ending::Cancel { from: status } => {
                // No request drives a pass, so the status the cancel is to arrive in is
                // written into the store — as `contact_cancellation.rs` does, for the same
                // reason.
                put_in_status(&engine, &contact_id, status);
                if matches!(row.ending, Ending::Cancel { .. }) {
                    let cancelled =
                        send(&router, "DELETE", &format!("/contact/{contact_id}"), None).await;
                    assert_eq!(
                        cancelled.status, 200,
                        "case {}: CancelContact expected 200, got {} {}",
                        row.case, cancelled.status, cancelled.body
                    );
                }
            }
            Ending::Terminal(status) => put_in_status(&engine, &contact_id, status),
        }

        let before = describe_contact(&router, &row.case, &contact_id).await;
        let pending_version = before.field("version").clone();
        assert_eq!(
            pending_version.get("status").and_then(Value::as_str),
            Some(state::UPDATING_VERSION_STATUS),
            "case {}: the change is still in flight {:?} after it was appended, so the \
             newest version must read {}, got {pending_version}",
            row.case,
            started.elapsed(),
            state::UPDATING_VERSION_STATUS
        );
        prepared.push(Prepared {
            row,
            contact_id,
            before,
        });
    }

    // Long enough that every move the updates left in flight has fallen due.
    std::thread::sleep(AFTER_SETTLE);

    for Prepared {
        row,
        contact_id,
        before,
    } in &prepared
    {
        let case = &row.case;
        let newest_version = i64::from(state::FIRST_VERSION_ID)
            + i64::try_from(row.pending).expect("a pending count fits in i64");
        let after = describe_contact(&router, case, contact_id).await;
        let reported = after.field("version").clone();
        let history_before = history_standing(&version_history(&router, case, contact_id).await);

        if row.applied {
            // The control: a contact that still has a pass to fly takes the change.
            assert_eq!(
                reported.get("status").and_then(Value::as_str),
                Some(state::ACTIVE_VERSION_STATUS),
                "case {case}: a contact with a pass still to fly takes the change in flight, \
                 so its newest version must read {}, got {reported}",
                state::ACTIVE_VERSION_STATUS
            );
            assert!(
                reported.get("activated").is_some_and(|at| !at.is_null()),
                "case {case}: an applied version reports when it was activated, got {reported}"
            );
            assert_ne!(
                reported_last_updated(&after),
                reported_last_updated(before),
                "case {case}: applying a version is a change to THAT version, so its \
                 lastUpdated moves with it, got {}",
                reported_last_updated(&after)
            );
            continue;
        }

        // The rule: the contact has no pass left, so the change in flight is withdrawn —
        // the version stays where the ending left it and never becomes the active one.
        assert_eq!(
            version_standing(&reported),
            version_standing(before.field("version")),
            "case {case}: a contact with no pass left takes no version change, so its \
             newest version must stand exactly as the ending left it, got {reported} after {}",
            before.field("version")
        );
        assert_eq!(
            reported.get("status").and_then(Value::as_str),
            Some(state::UPDATING_VERSION_STATUS),
            "case {case}: the withdrawn version is never applied, so it keeps the {} it was \
             appended in, got {reported}",
            state::UPDATING_VERSION_STATUS
        );
        assert!(
            reported.get("activated").is_none_or(Value::is_null),
            "case {case}: a version that was never applied reports no activation, got \
             {reported}"
        );

        // The version before it was never replaced: with nothing taking force, nothing is
        // superseded.
        let first = described_version(&router, case, contact_id, i64::from(state::FIRST_VERSION_ID))
            .await;
        assert_eq!(
            first.get("status").and_then(Value::as_str),
            Some(state::ACTIVE_VERSION_STATUS),
            "case {case}: no version took force, so version {} is still the active one, got \
             {first}",
            state::FIRST_VERSION_ID
        );
        assert!(
            first.get("superseded").is_none_or(Value::is_null),
            "case {case}: nothing superseded version {}, got {first}",
            state::FIRST_VERSION_ID
        );

        // Every reader of the version agrees, and the newest record of the history is the
        // one `DescribeContact` reports.
        assert_eq!(
            version_standing(history_before.last().unwrap_or(&Value::Null)),
            version_standing(&reported),
            "case {case}: the newest record of the history is the version the contact \
             reports, got {:?}",
            history_before.last()
        );
        assert_eq!(
            version_standing(&described_version(&router, case, contact_id, newest_version).await),
            version_standing(&reported),
            "case {case}: DescribeContactVersion must report version {newest_version} as the \
             contact does, got {reported}"
        );

        // A contact released before its pass, or one that can never leave the status it
        // holds, is owed NO further move at all: its lastUpdated stands where the ending
        // left it. A stop still under way is the exception — finishing the release is the
        // contact's own owed move — so that row is asserted on its status instead.
        let settling = before.string("contactStatus") == state::CONTACT_CANCELLING;
        if settling {
            assert_eq!(
                after.string("contactStatus"),
                state::CONTACT_CANCELLED,
                "case {case}: the release the cancel started is the one move that survives \
                 it, got {}",
                after.body
            );
        } else {
            assert_eq!(
                after.string("contactStatus"),
                before.string("contactStatus"),
                "case {case}: a contact with no pass left is owed no further move, so its \
                 status stands where the ending left it, got {}",
                after.body
            );
            assert_eq!(
                reported_last_updated(&after),
                reported_last_updated(before),
                "case {case}: nothing moved the newest version after the ending, so its \
                 lastUpdated must stand at {}, got {}",
                reported_last_updated(before),
                reported_last_updated(&after)
            );
        }

        // And the whole history is still what it was: reading it again changes nothing.
        let history_after = history_standing(&version_history(&router, case, contact_id).await);
        assert_eq!(
            history_after, history_before,
            "case {case}: the version history of a contact with no pass left stands as the \
             ending left it, got {history_after:?}"
        );

        // The other half of the same question: what the service will not START on this
        // contact it does not finish either, so an update of it is refused.
        let refused = update_contact(
            &router,
            contact_id,
            json!({"satelliteArn": satellites[0]}),
        )
        .await;
        assert_eq!(
            refused.status, 431,
            "case {case}: expected the modelled InvalidParameterException status 431 for an \
             update of a contact with no pass left, got {} {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "InvalidParameterException",
            "case {case}: wrong error shape, got {}",
            refused.body
        );
    }
}
