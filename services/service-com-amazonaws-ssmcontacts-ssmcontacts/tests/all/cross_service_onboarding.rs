// SPDX-License-Identifier: CC0-1.0

//! One gate, one refusal: the five contact-registry writes cannot word the account-level
//! precondition differently.
//!
//! Amazon SSM Contacts is fronted by an account-level Incident Manager replication set.
//! Each operation has its OWN generated error enum, so each handler constructs its own
//! `ValidationException` — which is exactly how two operations fronted by the same
//! account-level resource could drift apart. This suite pins that they do not: for one
//! account state, every write's refusal carries the same `__type`, the same `Message`, the
//! same `Reason` and the same (empty) `Fields`, and every one of them flips to serviced
//! when — and only when — the account's record becomes ACTIVE.
//!
//! The CROSS-SERVICE half of this rule — that Incident Manager's own
//! `CreateReplicationSet` refuses every call as deprecated, so nothing onboards an
//! emulated account over the wire — is asserted where both services are mounted:
//! `core/router/tests/all/ssm_contacts_replication_set.rs`.

use serde_json::{json, Value};

use crate::contact_lifecycle::{error_code, Emulator, ACCOUNT, REGION};
use crate::contact_updates::{active_replication_set, set_replication_set};
use service_com_amazonaws_ssmcontacts_ssmcontacts::state::AccountReplicationSet;

/// The five registry writes, each with a request that is well formed on its own.
///
/// The bodies differ (they are different operations over different inputs); what must NOT
/// differ is the refusal they answer while the account has no ACTIVE replication set.
fn writes() -> Vec<(&'static str, Value)> {
    vec![
        (
            "CreateContact",
            json!({
                "Alias": "oncall-primary",
                "Type": "PERSONAL",
                "Plan": {"Stages": [{"DurationInMinutes": 5, "Targets": []}]},
            }),
        ),
        (
            "CreateContactChannel",
            json!({
                "ContactId": format!("arn:aws:ssm-contacts:{REGION}:{ACCOUNT}:contact/oncall-primary"),
                "Name": "email",
                "Type": "EMAIL",
                "DeliveryAddress": {"SimpleAddress": "ops@example.com"},
            }),
        ),
        (
            "UpdateContact",
            json!({
                "ContactId": format!("arn:aws:ssm-contacts:{REGION}:{ACCOUNT}:contact/oncall-primary"),
                "DisplayName": "On-call primary renamed",
            }),
        ),
        (
            "DeleteContactChannel",
            json!({
                "ContactChannelId": format!(
                    "arn:aws:ssm-contacts:{REGION}:{ACCOUNT}:contact-channel/oncall-primary/absent"
                ),
            }),
        ),
        (
            "DeleteContact",
            json!({
                "ContactId": format!("arn:aws:ssm-contacts:{REGION}:{ACCOUNT}:contact/oncall-primary"),
            }),
        ),
    ]
}

/// The four members the refusal is made of, as the wire reports them.
///
/// A named record rather than a tuple so the comparison below says which member differs.
#[derive(Debug, PartialEq, Eq)]
struct RefusalShape {
    code: String,
    message: String,
    reason: String,
    field_count: Option<usize>,
}

/// Read the refusal `body` answered into that record.
fn refusal_shape(body: &Value) -> RefusalShape {
    RefusalShape {
        code: error_code(body).to_string(),
        message: body["Message"].as_str().unwrap_or_default().to_string(),
        reason: body["Reason"].as_str().unwrap_or_default().to_string(),
        field_count: body["Fields"].as_array().map(Vec::len),
    }
}

/// Every write refuses a non-onboarded account with the SAME status and the same error
/// body, for both ways an account can lack an ACTIVE set.
#[tokio::test]
async fn the_five_registry_writes_word_the_account_refusal_identically() {
    let account_states: &[(&str, Option<AccountReplicationSet>)] = &[
        ("no replication set at all", None),
        (
            "a replication set that is not active yet",
            Some(AccountReplicationSet {
                active: false,
                ..active_replication_set()
            }),
        ),
    ];

    for (account_case, replication_set) in account_states {
        let emulator = Emulator::new();
        set_replication_set(&emulator, replication_set.clone());

        let mut seen: Option<(&'static str, RefusalShape)> = None;
        for (operation, request) in writes() {
            let case = format!("{account_case}: {operation}");
            let (status, body) = emulator.call(operation, request).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with {body:?}"
            );
            let shape = refusal_shape(&body);
            assert!(
                shape.code.contains("ValidationException"),
                "case {case:?}: expected the ValidationException code, got {:?}",
                shape.code
            );
            assert!(
                !shape.message.is_empty(),
                "case {case:?}: expected a populated Message, got {body:?}"
            );
            assert_eq!(
                shape.field_count,
                Some(0),
                "case {case:?}: no input member is at fault, so Fields must be present and \
                 empty, got {body:?}"
            );
            match &seen {
                None => seen = Some((operation, shape)),
                Some((first_operation, first)) => assert_eq!(
                    &shape, first,
                    "case {case:?}: one account-level gate must answer one refusal, but \
                     {operation} differs from {first_operation}"
                ),
            }
        }
    }
}

/// The same five writes stop being refused the moment the account's record is ACTIVE:
/// each one answers about its own subject (created, or 404/409 about the target) instead
/// of about the account.
#[tokio::test]
async fn the_same_writes_answer_about_their_own_subject_once_the_account_is_active() {
    let emulator = Emulator::new();
    set_replication_set(&emulator, Some(active_replication_set()));

    // Sent in order, so the rows that name `contact/oncall-primary` resolve the contact
    // the first row created and the channel the second one did.
    let expected: &[(&str, u16)] = &[
        ("CreateContact", 200),
        ("CreateContactChannel", 200),
        ("UpdateContact", 200),
        ("DeleteContactChannel", 404),
        ("DeleteContact", 409),
    ];

    for ((operation, request), (expected_operation, want)) in writes().into_iter().zip(expected) {
        assert_eq!(
            &operation, expected_operation,
            "the expectation table must stay aligned with the write table"
        );
        let (status, body) = emulator.call(operation, request).await;
        assert_eq!(
            status, *want,
            "case {operation:?}: expected HTTP {want} on an ACTIVE account, got {status} \
             with {body:?}"
        );
        assert!(
            !error_code(&body).contains("ValidationException"),
            "case {operation:?}: an ACTIVE account must not be refused by the account gate, \
             got {body:?}"
        );
    }
}
