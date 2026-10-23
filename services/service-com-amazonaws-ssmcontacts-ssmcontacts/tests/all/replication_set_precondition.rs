// SPDX-License-Identifier: CC0-1.0

//! The ACCOUNT-level replication-set precondition over the contact REGISTRY's writes,
//! end to end over the wire.
//!
//! The rule under test:
//!
//! > A call that creates or changes a contact resource is refused with the operation's own
//! > modelled `ValidationException` (HTTP 400), BEFORE the resource it names is resolved
//! > and without writing anything, whenever the account has no ACTIVE Incident Manager
//! > replication set — and is serviced once the account has one.
//!
//! Amazon SSM Contacts is a global service whose contacts, escalation plans and on-call
//! schedules live in the Regions of that per-account set, so before one exists there is no
//! home Region to put the resource in. `tests/all/data_plane_precondition.rs` asserts the
//! same gate over the rotation/engagement/policy/tag slice; this suite is about the five
//! registry writes (`CreateContact`, `CreateContactChannel`, `UpdateContact`,
//! `DeleteContact`, `DeleteContactChannel`).
//!
//! Each suite is a TABLE spanning what the rule generalises over — which write is called,
//! and the shape of its request (contact type, both shapes the modelled `Plan` admits,
//! whether `IdempotencyToken`/`Tags`/`DisplayName` are named, the channel type, a target
//! that resolves and a target that does not) — rather than the one `CreateContact` request
//! the wire corpus recorded, which is one row among them. The refusal is about the ACCOUNT,
//! so no row's own input can change the answer.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use service_com_amazonaws_ssmcontacts_ssmcontacts::ops::types::VALIDATION_EXCEPTION_REASON_VALUES;

use crate::contact_lifecycle::{error_code, Emulator, ACCOUNT, REGION};
use crate::contact_updates::{active_replication_set, onboard, set_replication_set};
use service_com_amazonaws_ssmcontacts_ssmcontacts::state::AccountReplicationSet;

/// One row of the write table: an operation and the request body to send it.
struct WriteRow {
    case: &'static str,
    operation: &'static str,
    request: Value,
}

/// Every implemented registry write, over request shapes that differ in what the rule
/// generalises over: all three `ContactType` values, both shapes the modelled `Plan`
/// admits (`Stages` and `RotationIds`), with and without `IdempotencyToken`/`Tags`/
/// `DisplayName`, two `ChannelType` values, and a target ARN that resolves versus one that
/// does not.
///
/// The rows are sent IN ORDER, so on an onboarded account the rows that name
/// `contact/oncall-primary` resolve the contact the first row created — which is how the
/// second suite tells "refused because of the account" apart from "404 because of the
/// target". Before onboarding the order makes no difference: nothing is created at all.
fn write_rows() -> Vec<WriteRow> {
    vec![
        WriteRow {
            case: "CreateContact for a PERSONAL contact with an engagement plan",
            operation: "CreateContact",
            request: json!({
                "Alias": "oncall-primary",
                "DisplayName": "On-call primary",
                "Type": "PERSONAL",
                "Plan": {"Stages": [{"DurationInMinutes": 5, "Targets": []}]},
            }),
        },
        WriteRow {
            case: "CreateContact for an ESCALATION plan with two stages",
            operation: "CreateContact",
            request: json!({
                "Alias": "escalation-plan",
                "Type": "ESCALATION",
                "Plan": {"Stages": [
                    {"DurationInMinutes": 0, "Targets": []},
                    {"DurationInMinutes": 30, "Targets": []},
                ]},
            }),
        },
        WriteRow {
            case: "CreateContact for an ONCALL_SCHEDULE whose plan carries rotation ids",
            operation: "CreateContact",
            request: json!({
                "Alias": "weekend-rotation",
                "Type": "ONCALL_SCHEDULE",
                "Plan": {"RotationIds": [
                    format!("arn:aws:ssm-contacts:{REGION}:{ACCOUNT}:rotation/weekend"),
                ]},
            }),
        },
        WriteRow {
            case: "CreateContact carrying an IdempotencyToken and tags",
            operation: "CreateContact",
            request: json!({
                "Alias": "tagged-contact",
                "Type": "PERSONAL",
                "Plan": {"Stages": []},
                "IdempotencyToken": "token-precondition-1",
                "Tags": [{"Key": "team", "Value": "sre"}],
            }),
        },
        WriteRow {
            case: "CreateContactChannel of type EMAIL under a contact that exists once onboarded",
            operation: "CreateContactChannel",
            request: json!({
                "ContactId": format!("arn:aws:ssm-contacts:{REGION}:{ACCOUNT}:contact/oncall-primary"),
                "Name": "email",
                "Type": "EMAIL",
                "DeliveryAddress": {"SimpleAddress": "ops@example.com"},
            }),
        },
        WriteRow {
            case: "CreateContactChannel of type SMS under a contact that never exists",
            operation: "CreateContactChannel",
            request: json!({
                "ContactId": format!("arn:aws:ssm-contacts:{REGION}:{ACCOUNT}:contact/absent"),
                "Name": "sms",
                "Type": "SMS",
                "DeliveryAddress": {"SimpleAddress": "+15550100"},
            }),
        },
        WriteRow {
            case: "UpdateContact renaming a contact that exists once onboarded",
            operation: "UpdateContact",
            request: json!({
                "ContactId": format!("arn:aws:ssm-contacts:{REGION}:{ACCOUNT}:contact/oncall-primary"),
                "DisplayName": "On-call primary renamed",
            }),
        },
        WriteRow {
            case: "UpdateContact replacing the plan of a contact that never exists",
            operation: "UpdateContact",
            request: json!({
                "ContactId": format!("arn:aws:ssm-contacts:{REGION}:{ACCOUNT}:contact/absent"),
                "Plan": {"Stages": [{"DurationInMinutes": 10, "Targets": []}]},
            }),
        },
        WriteRow {
            case: "DeleteContactChannel naming a channel that never exists",
            operation: "DeleteContactChannel",
            request: json!({
                "ContactChannelId": format!(
                    "arn:aws:ssm-contacts:{REGION}:{ACCOUNT}:contact-channel/oncall-primary/absent"
                ),
            }),
        },
        WriteRow {
            case: "DeleteContact naming a contact that exists once onboarded",
            operation: "DeleteContact",
            request: json!({
                "ContactId": format!("arn:aws:ssm-contacts:{REGION}:{ACCOUNT}:contact/oncall-primary"),
            }),
        },
        WriteRow {
            case: "DeleteContact naming a contact that never exists",
            operation: "DeleteContact",
            request: json!({
                "ContactId": format!("arn:aws:ssm-contacts:{REGION}:{ACCOUNT}:contact/absent"),
            }),
        },
    ]
}

/// Every registry write is refused with the modelled `ValidationException` while the
/// account has no ACTIVE replication set — whatever the request names, whether or not the
/// resource it names would exist, and for BOTH ways an account can lack one (no set at
/// all, and a set that has not finished replicating).
#[tokio::test]
async fn registry_writes_are_refused_while_the_account_has_no_active_replication_set() {
    let no_active_set: &[(&str, Option<AccountReplicationSet>)] = &[
        ("no replication set at all", None),
        (
            "a replication set that is not active yet",
            Some(AccountReplicationSet {
                active: false,
                ..active_replication_set()
            }),
        ),
    ];

    for (account_case, replication_set) in no_active_set {
        let emulator = Emulator::new();
        set_replication_set(&emulator, replication_set.clone());

        for row in write_rows() {
            let (status, body) = emulator.call(row.operation, row.request.clone()).await;
            let case = format!("{account_case}: {}", row.case);

            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400 for an account with no active replication \
                 set, got {status} with {body:?}"
            );
            assert!(
                error_code(&body).contains("ValidationException"),
                "case {case:?}: expected the error code ValidationException — NOT a \
                 ResourceNotFoundException, even where the target does not exist — got {:?} \
                 in {body:?}",
                error_code(&body)
            );
            let message = body["Message"].as_str().unwrap_or_default();
            // An invariant of the message, not a transcript: the refusal reports the
            // ACCOUNT, so it must say so and must NOT blame a member of the request — the
            // remedy for the two is different.
            assert!(
                message.contains("Account"),
                "case {case:?}: expected a Message reporting the account-level precondition, \
                 got {message:?} in {body:?}"
            );
            for member in ["Alias", "Plan", "ContactId", "ContactChannelId"] {
                assert!(
                    !message.contains(member),
                    "case {case:?}: an account-level refusal must not blame {member}, got \
                     {message:?} in {body:?}"
                );
            }
            // The recorded AWS response for this precondition carried a `Reason` outside
            // the modelled `ValidationExceptionReason` set, so what is pinned here is that
            // the emulator stays INSIDE it (the crate's own model-derived list, imported
            // rather than copied).
            let reason = body["Reason"].as_str().unwrap_or_default();
            assert!(
                VALIDATION_EXCEPTION_REASON_VALUES.contains(&reason),
                "case {case:?}: expected a Reason inside the modelled \
                 ValidationExceptionReason set {VALIDATION_EXCEPTION_REASON_VALUES:?}, got \
                 {reason:?} in {body:?}"
            );
            assert_eq!(
                body["Fields"].as_array().map(Vec::len),
                Some(0),
                "case {case:?}: no input member is at fault, so Fields must be present and \
                 empty, got {body:?}"
            );
        }

        // A refusal is not a partial write: once the account IS onboarded, nothing the
        // refused table named is there.
        onboard(&emulator);
        let (status, body) = emulator.call("ListContacts", json!({})).await;
        assert_eq!(
            status, 200,
            "case {account_case:?}: expected ListContacts to answer 200 on the onboarded \
             account, got {status} with {body:?}"
        );
        assert_eq!(
            body["Contacts"].as_array().map(Vec::len).unwrap_or_default(),
            0,
            "case {account_case:?}: every write was refused, so no contact may be persisted, \
             got {body:?}"
        );
    }
}

/// The SAME rows are serviced once the account has an ACTIVE replication set: the refusal
/// is a precondition on the account, not on any request this suite sends.
#[tokio::test]
async fn the_same_writes_are_serviced_once_the_account_has_a_replication_set() {
    let emulator = Emulator::new();
    onboard(&emulator);

    // The creates that name only themselves succeed; the rows that resolve another
    // resource answer about THAT resource instead of about the account — 404 for a target
    // that does not exist, 409 for the contact whose channel the earlier row created.
    let expected: BTreeMap<&str, u16> = BTreeMap::from([
        ("CreateContact for a PERSONAL contact with an engagement plan", 200),
        ("CreateContact for an ESCALATION plan with two stages", 200),
        ("CreateContact for an ONCALL_SCHEDULE whose plan carries rotation ids", 200),
        ("CreateContact carrying an IdempotencyToken and tags", 200),
        ("CreateContactChannel of type EMAIL under a contact that exists once onboarded", 200),
        ("CreateContactChannel of type SMS under a contact that never exists", 404),
        ("UpdateContact renaming a contact that exists once onboarded", 200),
        ("UpdateContact replacing the plan of a contact that never exists", 404),
        ("DeleteContactChannel naming a channel that never exists", 404),
        ("DeleteContact naming a contact that exists once onboarded", 409),
        ("DeleteContact naming a contact that never exists", 404),
    ]);

    for row in write_rows() {
        let case = row.case;
        let want = *expected
            .get(case)
            .unwrap_or_else(|| panic!("case {case:?}: the table must name an expected status"));
        let (status, body) = emulator.call(row.operation, row.request.clone()).await;
        assert_eq!(
            status, want,
            "case {case:?}: expected HTTP {want} on an onboarded account, got {status} \
             with {body:?}"
        );
    }
}
