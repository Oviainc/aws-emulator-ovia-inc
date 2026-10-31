// SPDX-License-Identifier: CC0-1.0

//! Tests for the instant a Firewall Manager WRITE is decided at and stamped
//! with.
//!
//! The rule asserted here, in one sentence: a handler that decides from a
//! clock-derived reading of the administrator association and then persists a
//! record carrying an instant takes ONE reading of the clock for the whole
//! request, so the instant it stores is the instant it decided from — never a
//! second, independent reading taken after the decision.
//!
//! The classes the rule ranges over:
//!
//! * the WRITE SITE: the organisation's single administrator slot
//!   (`AssociateAdminAccount`, whose record carries when the association was
//!   made) and the per-Region notification channel (`PutNotificationChannel`,
//!   whose record carries when the channel was last designated) — the two
//!   handlers of this service that decide from the clock and then stamp a
//!   record;
//! * the STATE of the association the decision is made against: no association
//!   at all, one that is still an administrator, one whose requested tear-down
//!   completes later, one whose tear-down has already completed, and one whose
//!   role is already deleted — crossed, for the administrator slot, with whether
//!   the request names the account that holds it or another one;
//! * for the channel, the designated VALUES, taken from the shared value class
//!   in [`crate::notification_channel_deletion::cases`] (both published bounds of
//!   `com.amazonaws.fms#ResourceArn`, differing in Region segment, role path form
//!   and punctuation), so the instant a write records cannot depend on the shape
//!   of its payload.
//!
//! Every call is driven over the WIRE through the awsJson1_1 harness in
//! [`crate::policy_lifecycle`], and the instant each write recorded is then read
//! off the persisted record: a stamp must fall inside the window the request
//! itself occupied, and the decision that request made must hold AT that stamp —
//! the association it replaced must not be an administrator then, and the
//! association that authorized it must be one then.
//!
//! What these assertions can and cannot reach: this crate's handlers read the
//! clock directly, so a test has no seam to hand them an instant through, and
//! the divergence a SECOND reading produces needs a clock that steps backwards
//! between the two. What is asserted here is therefore the invariant the
//! single-reading discipline guarantees — the stamp a write records is an instant
//! inside the request that the request's own decision holds at — together with
//! the full modelled outcome of each row.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::json;
use service_com_amazonaws_fms_awsfms_20180101::state;

use crate::administrator_gate_clock_reading::association;
use crate::administrator_gated_operations::{error_message, with_store};
use crate::notification_channel_deletion::cases;
use crate::policy_lifecycle::{call, emulator_with_engine, error_code, ACCOUNT, REGION};

/// The error an association the organisation cannot make is refused with.
const INVALID_OPERATION: &str = "InvalidOperationException";

/// The error code an organisation-scoped call answers when no administrator
/// association is active at the instant it decides from.
const ACCESS_DENIED: &str = "AccessDeniedException";

/// An account that is not the caller, so a row can ask for an association of
/// another account than the one holding the slot. The model constrains
/// `AWSAccountId` to `^[0-9]+$`, length 1..1024.
const OTHER_ACCOUNT: &str = "123456789012";

/// What the handler must do with the request, given the state of the slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expected {
    /// The slot was free at the request's instant, so a fresh association of the
    /// requested account is written.
    Written,
    /// The requested account already holds the slot, so there is nothing to
    /// change and the stored record — its stamp included — is left exactly as it
    /// was.
    Untouched,
    /// The association cannot be made, so the call is refused and writes
    /// nothing.
    Refused,
}

/// One row: the association the slot holds when the request arrives, and the
/// account the request asks to associate.
struct Row {
    name: &'static str,
    /// The role the seeded association carries, and where the completion instant
    /// of a requested tear-down sits relative to the request, in milliseconds
    /// (`None` for no tear-down asked for). `None` for the whole pair means the
    /// slot holds no association at all.
    seeded: Option<(&'static str, Option<i64>)>,
    requested: &'static str,
    expected: Expected,
}

/// The table: every state the administrator slot can be in when a writer decides
/// against it, crossed with whether the request names the holder or another
/// account.
fn rows() -> Vec<Row> {
    vec![
        Row {
            name: "no association at all, the caller's own account",
            seeded: None,
            requested: ACCOUNT,
            expected: Expected::Written,
        },
        Row {
            name: "no association at all, another account",
            seeded: None,
            requested: OTHER_ACCOUNT,
            expected: Expected::Written,
        },
        Row {
            name: "the same account, no tear-down requested",
            seeded: Some((state::ROLE_STATUS_READY, None)),
            requested: ACCOUNT,
            expected: Expected::Untouched,
        },
        Row {
            name: "a different account, no tear-down requested",
            seeded: Some((state::ROLE_STATUS_READY, None)),
            requested: OTHER_ACCOUNT,
            expected: Expected::Refused,
        },
        Row {
            name: "the same account, a tear-down completing in an hour",
            seeded: Some((state::ROLE_STATUS_PENDING_DELETION, Some(3_600_000))),
            requested: ACCOUNT,
            expected: Expected::Refused,
        },
        Row {
            name: "a different account, a role being deleted, completing in half a minute",
            seeded: Some((state::ROLE_STATUS_DELETING, Some(30_000))),
            requested: OTHER_ACCOUNT,
            expected: Expected::Refused,
        },
        Row {
            name: "the same account, a tear-down that completed a millisecond ago",
            seeded: Some((state::ROLE_STATUS_PENDING_DELETION, Some(-1))),
            requested: ACCOUNT,
            expected: Expected::Written,
        },
        Row {
            name: "a different account, a tear-down that completed an hour ago",
            seeded: Some((state::ROLE_STATUS_DELETING, Some(-3_600_000))),
            requested: OTHER_ACCOUNT,
            expected: Expected::Written,
        },
        Row {
            name: "a role already deleted",
            seeded: Some((state::ROLE_STATUS_DELETED, None)),
            requested: ACCOUNT,
            expected: Expected::Written,
        },
        Row {
            name: "a role already deleted, with a completion instant still ahead",
            seeded: Some((state::ROLE_STATUS_DELETED, Some(3_600_000))),
            requested: OTHER_ACCOUNT,
            expected: Expected::Written,
        },
    ]
}

/// `now` shifted by `millis`, which may be negative.
fn shifted(now: SystemTime, millis: i64) -> SystemTime {
    let magnitude = Duration::from_millis(millis.unsigned_abs());
    if millis >= 0 {
        now + magnitude
    } else {
        now - magnitude
    }
}

/// The association the store holds, cloned.
fn stored_association(
    engine: &Arc<engine::InMemoryEngine>,
) -> Option<state::AdminAccountRecord> {
    with_store(engine, |store| store.admin_account.clone())
}

/// Whether the administrator gate would admit an organisation-scoped call made
/// at `now`, decided against the store as it is NOW (so the seeded association's
/// own clock-derived predicate decides it).
fn gate_admits(engine: &Arc<engine::InMemoryEngine>, now: SystemTime) -> bool {
    with_store(engine, |store| {
        state::require_administrator_at::<()>(store, REGION, now).is_ok()
    })
}

/// A writer of the administrator slot stamps the record it persists with the
/// instant it DECIDED from: the stamp falls inside the window the request
/// occupied, the association it replaced is no longer an administrator at that
/// stamp, and the association it wrote already is one. A request that changes
/// nothing — because the account it names already holds the slot — leaves the
/// stamp it found, and a refused one leaves the whole record alone.
#[tokio::test]
async fn an_association_is_stamped_with_the_instant_it_was_decided_from() {
    for row in rows() {
        let name = row.name;
        let (router, engine) = emulator_with_engine();
        // Seeded through the ONE builder this test target shares for an
        // association with a tear-down in flight, so its stamp is an hour before
        // the request: far outside the window a request occupies, which is what
        // tells a record left untouched apart from one re-stamped.
        let seeded = row.seeded.map(|(role_status, completes_in_millis)| {
            association(
                role_status,
                completes_in_millis.map(|millis| shifted(SystemTime::now(), millis)),
            )
        });
        let seeded_at = seeded.as_ref().map(|record| record.associated_at);
        if let Some(record) = seeded.clone() {
            with_store(&engine, |store| store.admin_account = Some(record));
        }

        let before = SystemTime::now();
        let (status, body) = call(
            &router,
            "AssociateAdminAccount",
            &json!({ "AdminAccount": row.requested }),
        )
        .await;
        let after = SystemTime::now();

        match row.expected {
            Expected::Refused => {
                assert_eq!(
                    (status, error_code(&body).as_str()),
                    (400, INVALID_OPERATION),
                    "case {name:?}: an association the organisation cannot make must be refused, \
                     got {status} with {body}"
                );
                let message = error_message(&body);
                assert!(
                    message.contains(ACCOUNT),
                    "case {name:?}: the refusal must name the account holding the slot, got \
                     {message:?}"
                );
                let got = stored_association(&engine);
                assert_eq!(
                    got.as_ref().map(|record| (
                        record.account_id.clone(),
                        record.role_status.clone(),
                        record.associated_at,
                        record.removal_due
                    )),
                    seeded.as_ref().map(|record| (
                        record.account_id.clone(),
                        record.role_status.clone(),
                        record.associated_at,
                        record.removal_due
                    )),
                    "case {name:?}: a refused association must leave the record exactly as it \
                     was, got {got:?}"
                );
            }
            Expected::Untouched => {
                assert_eq!(
                    status, 200,
                    "case {name:?}: re-associating the account that already holds the slot must \
                     answer 200, got {status} with {body}"
                );
                let got = stored_association(&engine);
                let stamp = got.as_ref().map(|record| record.associated_at);
                assert_eq!(
                    stamp, seeded_at,
                    "case {name:?}: a request that changes nothing must not re-stamp the record, \
                     got {stamp:?} for a record associated at {seeded_at:?}"
                );
                assert_eq!(
                    got.as_ref().map(|record| record.role_status.as_str()),
                    Some(state::ROLE_STATUS_READY),
                    "case {name:?}: the existing association must be left as it was, got {got:?}"
                );
                assert_reported(&router, name, ACCOUNT).await;
            }
            Expected::Written => {
                assert_eq!(
                    status, 200,
                    "case {name:?}: an association of a free slot must answer 200, got {status} \
                     with {body}"
                );
                let got = stored_association(&engine)
                    .unwrap_or_else(|| panic!("case {name:?}: nothing was written"));
                assert_eq!(
                    (got.account_id.as_str(), got.role_status.as_str(), got.removal_due),
                    (row.requested, state::ROLE_STATUS_READY, None),
                    "case {name:?}: a fresh association of {} must be READY with no tear-down \
                     pending, got {got:?}",
                    row.requested
                );

                // The stamp is an instant inside the request, not one taken
                // before it was decided or after it was answered.
                let stamp = got.associated_at;
                assert!(
                    stamp >= before && stamp <= after,
                    "case {name:?}: expected the record stamped inside the request's own window \
                     {before:?}..={after:?}, got {stamp:?}"
                );

                // And the decision the request made holds AT that stamp: the
                // association it wrote is an administrator then, and the one it
                // replaced is not.
                assert!(
                    got.is_active_at(stamp),
                    "case {name:?}: the association written must be an administrator at the \
                     instant it records, got {got:?}"
                );
                assert!(
                    gate_admits(&engine, stamp),
                    "case {name:?}: the administrator gate must admit a call made at the instant \
                     the association records, got refused for {got:?}"
                );
                if let Some(replaced) = seeded.as_ref() {
                    assert!(
                        !replaced.is_active_at(stamp),
                        "case {name:?}: the slot was taken to be free, so the association \
                         replaced must not be an administrator at the stamped instant {stamp:?}, \
                         got {replaced:?}"
                    );
                    // Only where the slot was freed BY THE CLOCK: a role already
                    // deleted is free whatever instant a tear-down of it was
                    // still due at.
                    if let Some(due) = replaced
                        .removal_due
                        .filter(|_| replaced.role_status != state::ROLE_STATUS_DELETED)
                    {
                        assert!(
                            stamp >= due,
                            "case {name:?}: the replaced tear-down completed at {due:?}, so the \
                             instant the write decided from cannot be before it, got {stamp:?}"
                        );
                    }
                }
                assert_reported(&router, name, row.requested).await;
            }
        }
    }
}

/// The `AccountRoleStatus` a live association is reported with, resolved against
/// the model's own catalog (`com.amazonaws.fms#AccountRoleStatus`, exposed as
/// `ops::types::ACCOUNT_ROLE_STATUS_VALUES`) so the expectation names a value the
/// model really carries.
fn reported_ready() -> &'static str {
    service_com_amazonaws_fms_awsfms_20180101::ops::types::ACCOUNT_ROLE_STATUS_VALUES
        .iter()
        .copied()
        .find(|value| *value == state::ROLE_STATUS_READY)
        .expect("READY is one of the modelled AccountRoleStatus values")
}

/// The association the service REPORTS after the call: the write read back
/// through `GetAdminAccount`, so a record the handler persisted is one a client
/// can see.
async fn assert_reported(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    account: &str,
) {
    let (status, body) = call(router, "GetAdminAccount", &json!({})).await;
    assert_eq!(
        (
            status,
            body["AdminAccount"].as_str(),
            body["RoleStatus"].as_str()
        ),
        (200, Some(account), Some(reported_ready())),
        "case {case:?}: the administrator the call left must be reported READY, got {status} with \
         {body}"
    );
}

/// One tear-down state of the association a channel designation is gated by.
struct ChannelRow {
    name: &'static str,
    role_status: &'static str,
    /// Where the completion instant of a requested tear-down sits relative to the
    /// request, in milliseconds; `None` when no tear-down was requested.
    completes_in_millis: Option<i64>,
    /// Whether the association is still an administrator at the request's
    /// instant, so the designation is served.
    served: bool,
}

/// The states of the association the channel writer decides against: served and
/// refused rows, spanning the roles the model's `AccountRoleStatus` carries for a
/// tear-down in flight and one already completed.
fn channel_rows() -> Vec<ChannelRow> {
    vec![
        ChannelRow {
            name: "no tear-down requested",
            role_status: state::ROLE_STATUS_READY,
            completes_in_millis: None,
            served: true,
        },
        ChannelRow {
            name: "a tear-down completing in an hour",
            role_status: state::ROLE_STATUS_PENDING_DELETION,
            completes_in_millis: Some(3_600_000),
            served: true,
        },
        ChannelRow {
            name: "a role being deleted, completing in half a minute",
            role_status: state::ROLE_STATUS_DELETING,
            completes_in_millis: Some(30_000),
            served: true,
        },
        ChannelRow {
            name: "a tear-down that completed a millisecond ago",
            role_status: state::ROLE_STATUS_PENDING_DELETION,
            completes_in_millis: Some(-1),
            served: false,
        },
        ChannelRow {
            name: "a role already deleted",
            role_status: state::ROLE_STATUS_DELETED,
            completes_in_millis: None,
            served: false,
        },
    ]
}

/// The same rule at the other write site: a designated notification channel is
/// stamped with the instant its administrator gate admitted the call — inside the
/// window the request occupied, and an instant at which the association that
/// authorized it IS an administrator — and a designation the gate refuses stores
/// nothing at all, whatever values it carried.
#[tokio::test]
async fn a_designated_channel_is_stamped_with_the_instant_its_gate_admitted_the_call() {
    // Each row of the association class paired with one row of the shared
    // `ResourceArn` value class, so the stamp cannot depend on the payload. The
    // pairing is total, so neither table can grow a row the other silently drops.
    assert_eq!(
        channel_rows().len(),
        cases().len(),
        "every association state must be paired with a designated value class, got {} states for \
         {} value rows",
        channel_rows().len(),
        cases().len()
    );
    for (row, values) in channel_rows().into_iter().zip(cases()) {
        let name = format!("{} / {}", row.name, values.name);
        let (router, engine) = emulator_with_engine();
        let removal_due = row
            .completes_in_millis
            .map(|millis| shifted(SystemTime::now(), millis));
        with_store(&engine, |store| {
            store.admin_account = Some(association(row.role_status, removal_due));
        });

        let before = SystemTime::now();
        let (status, body) = call(
            &router,
            "PutNotificationChannel",
            &json!({ "SnsTopicArn": values.topic, "SnsRoleName": values.role }),
        )
        .await;
        let after = SystemTime::now();

        let stored = with_store(&engine, |store| {
            store.notification_channels.get(REGION).cloned()
        });
        if !row.served {
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, ACCESS_DENIED),
                "case {name:?}: a designation made with no administrator at the request's instant \
                 must be refused, got {status} with {body}"
            );
            assert!(
                stored.is_none(),
                "case {name:?}: a refused designation must store nothing, got {stored:?}"
            );
            continue;
        }

        assert_eq!(
            status, 200,
            "case {name:?}: a designation an administrator made must answer 200, got {status} \
             with {body}"
        );
        let stored =
            stored.unwrap_or_else(|| panic!("case {name:?}: no channel was stored"));
        assert_eq!(
            (stored.sns_topic_arn.as_str(), stored.sns_role_name.as_str()),
            (values.topic.as_str(), values.role.as_str()),
            "case {name:?}: the stored channel must be the designated pair, got {stored:?}"
        );

        let stamp = stored.configured_at;
        assert!(
            stamp >= before && stamp <= after,
            "case {name:?}: expected the channel stamped inside the request's own window \
             {before:?}..={after:?}, got {stamp:?}"
        );
        assert!(
            gate_admits(&engine, stamp),
            "case {name:?}: the gate that admitted the designation must admit a call made at the \
             instant the channel records, got refused for {stored:?}"
        );
    }
}
