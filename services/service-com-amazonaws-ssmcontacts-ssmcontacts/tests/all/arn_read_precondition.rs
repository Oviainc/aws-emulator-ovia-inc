// SPDX-License-Identifier: CC0-1.0

//! The ACCOUNT-level replication-set precondition over the contact plane's ARN-ADDRESSED
//! READS, end to end over the wire.
//!
//! The rule under test:
//!
//! > A call that must resolve a replication-set-scoped resource ARN taken from its request
//! > is refused with the operation's own modelled `ValidationException` (HTTP 400) BEFORE
//! > that ARN is resolved — never with a not-found answer about the resource and never with
//! > a complaint about another member of the request — whenever the account has no ACTIVE
//! > Incident Manager replication set, and answers about the resource once the account has
//! > one. A call that resolves NO resource ARN is not gated this way.
//!
//! `tests/all/replication_set_precondition.rs` asserts the same gate over the registry's
//! writes and `tests/all/data_plane_precondition.rs` over the rotation/engagement/policy/tag
//! slice; this suite is about the three reads that take a resource ARN from the request —
//! `GetContact`, `GetContactChannel` and `ListContactChannels` — with the account-wide
//! `ListContacts` as the ungated control.
//!
//! Each suite is a TABLE spanning what the rule generalises over: which read is called,
//! whether the ARN it names resolves to a stored resource or to nothing, how many channels
//! the named contact holds, and which of the listing's optional members (`MaxResults`, a
//! `NextToken` this service issued, a `NextToken` it did not) the request carries. The
//! refusal is about the ACCOUNT, so no row's own input may change the answer.

use serde_json::{json, Value};

use service_com_amazonaws_ssmcontacts_ssmcontacts::ops::types::VALIDATION_EXCEPTION_REASON_VALUES;
use service_com_amazonaws_ssmcontacts_ssmcontacts::state::AccountReplicationSet;

use crate::contact_lifecycle::{error_code, Emulator, ACCOUNT, REGION};
use crate::contact_updates::{active_replication_set, onboard, set_replication_set};

/// What the fixture below stored, and the well-formed ARNs nothing ever created.
struct Fixture {
    /// A contact holding two channels.
    busy_contact: String,
    /// A contact holding none, so "no channels" cannot be confused with "refused".
    empty_contact: String,
    /// One of `busy_contact`'s channels.
    channel: String,
    /// A `NextToken` this service issued, from the first page of `busy_contact`'s channels.
    issued_token: String,
    absent_contact: String,
    absent_channel: String,
}

/// Onboard the account and store two contacts, one of them with two channels, then take a
/// `NextToken` off the first page of its channel listing.
async fn fixture(emulator: &Emulator) -> Fixture {
    onboard(emulator);
    let busy_contact = emulator
        .create_contact(
            json!({
                "Alias": "oncall-primary",
                "Type": "PERSONAL",
                "Plan": {"Stages": [{"DurationInMinutes": 5, "Targets": []}]},
            }),
            "the suite's contact with channels",
        )
        .await;
    let empty_contact = emulator
        .create_contact(
            json!({
                "Alias": "oncall-secondary",
                "Type": "PERSONAL",
                "Plan": {"Stages": []},
            }),
            "the suite's contact without channels",
        )
        .await;

    let mut channels: Vec<String> = Vec::new();
    for (name, channel_type, address) in [
        ("email", "EMAIL", "ops@example.com"),
        ("sms", "SMS", "+15550100"),
    ] {
        let (status, body) = emulator
            .call(
                "CreateContactChannel",
                json!({
                    "ContactId": busy_contact,
                    "Name": name,
                    "Type": channel_type,
                    "DeliveryAddress": {"SimpleAddress": address},
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "the fixture's {channel_type} channel must be created, got {status} {body:?}"
        );
        channels.push(
            body["ContactChannelArn"]
                .as_str()
                .unwrap_or_else(|| panic!("expected a ContactChannelArn, got {body:?}"))
                .to_string(),
        );
    }

    let (status, body) = emulator
        .call(
            "ListContactChannels",
            json!({"ContactId": busy_contact, "MaxResults": 1}),
        )
        .await;
    assert_eq!(
        status, 200,
        "the fixture's first channel page must be served, got {status} {body:?}"
    );
    let issued_token = body["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("expected the fixture's first page to carry a NextToken, got {body:?}"))
        .to_string();

    Fixture {
        busy_contact,
        empty_contact,
        channel: channels
            .first()
            .expect("the fixture created two channels")
            .clone(),
        issued_token,
        absent_contact: format!("arn:aws:ssm-contacts:{REGION}:{ACCOUNT}:contact/never-created"),
        absent_channel: format!(
            "arn:aws:ssm-contacts:{REGION}:{ACCOUNT}:contact-channel/never-created/00000000-0000-0000-0000-000000000000"
        ),
    }
}

/// One row of the read table.
struct ReadRow {
    case: String,
    operation: &'static str,
    request: Value,
    /// The resource ARN the request names — what an ARN-shaped refusal may report.
    subject: String,
    /// What the SAME request answers once the account is onboarded.
    onboarded_status: u16,
}

/// Every ARN-addressed read, over request shapes differing in what the rule generalises
/// over: the operation, whether the ARN resolves, how many channels the contact holds, and
/// which optional members the listing carries.
fn read_rows(fixture: &Fixture) -> Vec<ReadRow> {
    vec![
        ReadRow {
            case: "GetContact on a stored contact".to_string(),
            operation: "GetContact",
            request: json!({"ContactId": fixture.busy_contact}),
            subject: fixture.busy_contact.clone(),
            onboarded_status: 200,
        },
        ReadRow {
            case: "GetContact on a contact nothing created".to_string(),
            operation: "GetContact",
            request: json!({"ContactId": fixture.absent_contact}),
            subject: fixture.absent_contact.clone(),
            onboarded_status: 404,
        },
        ReadRow {
            case: "GetContactChannel on a stored channel".to_string(),
            operation: "GetContactChannel",
            request: json!({"ContactChannelId": fixture.channel}),
            subject: fixture.channel.clone(),
            onboarded_status: 200,
        },
        ReadRow {
            case: "GetContactChannel on a channel nothing created".to_string(),
            operation: "GetContactChannel",
            request: json!({"ContactChannelId": fixture.absent_channel}),
            subject: fixture.absent_channel.clone(),
            onboarded_status: 404,
        },
        ReadRow {
            case: "ListContactChannels for a contact holding two channels".to_string(),
            operation: "ListContactChannels",
            request: json!({"ContactId": fixture.busy_contact}),
            subject: fixture.busy_contact.clone(),
            onboarded_status: 200,
        },
        ReadRow {
            case: "ListContactChannels for a contact holding none".to_string(),
            operation: "ListContactChannels",
            request: json!({"ContactId": fixture.empty_contact}),
            subject: fixture.empty_contact.clone(),
            onboarded_status: 200,
        },
        ReadRow {
            case: "ListContactChannels with MaxResults naming one channel per page".to_string(),
            operation: "ListContactChannels",
            request: json!({"ContactId": fixture.busy_contact, "MaxResults": 1}),
            subject: fixture.busy_contact.clone(),
            onboarded_status: 200,
        },
        ReadRow {
            case: "ListContactChannels resuming from a NextToken this service issued"
                .to_string(),
            operation: "ListContactChannels",
            request: json!({
                "ContactId": fixture.busy_contact,
                "MaxResults": 1,
                "NextToken": fixture.issued_token,
            }),
            subject: fixture.busy_contact.clone(),
            onboarded_status: 200,
        },
        ReadRow {
            // The gate precedes the token codec too: onboarded, this token is refused as a
            // member; un-onboarded, the answer is about the account instead.
            case: "ListContactChannels carrying a NextToken this service never issued"
                .to_string(),
            operation: "ListContactChannels",
            request: json!({
                "ContactId": fixture.busy_contact,
                "NextToken": "not-a-token-this-service-issued",
            }),
            subject: fixture.busy_contact.clone(),
            onboarded_status: 400,
        },
        ReadRow {
            case: "ListContactChannels for a contact nothing created".to_string(),
            operation: "ListContactChannels",
            request: json!({"ContactId": fixture.absent_contact}),
            subject: fixture.absent_contact.clone(),
            onboarded_status: 404,
        },
    ]
}

/// Every ARN-addressed read is refused with the modelled `ValidationException` while the
/// account has no ACTIVE replication set — whatever the request names, whether or not the
/// ARN would resolve, and for BOTH ways an account can lack one — and the refused read
/// changes nothing a later caller sees.
#[tokio::test]
async fn every_arn_addressed_read_is_refused_while_the_account_has_no_active_replication_set() {
    let no_active_set: &[(&str, Option<AccountReplicationSet>)] = &[
        ("no replication set at all", None),
        (
            "a replication set that has not finished replicating",
            Some(AccountReplicationSet {
                active: false,
                ..active_replication_set()
            }),
        ),
    ];

    for (account_case, replication_set) in no_active_set {
        let emulator = Emulator::new();
        let fixture = fixture(&emulator).await;
        let rows = read_rows(&fixture);

        // What each row answers on the ONBOARDED account, so the refusal below can be
        // compared against it: the gate must be the only thing that changed.
        let mut before: Vec<(u16, Value)> = Vec::new();
        for row in &rows {
            let case = format!("{account_case}: {} (onboarded)", row.case);
            let answer = emulator.call(row.operation, row.request.clone()).await;
            assert_eq!(
                answer.0, row.onboarded_status,
                "case {case:?}: expected HTTP {} on the onboarded account, got {} {:?}",
                row.onboarded_status, answer.0, answer.1
            );
            before.push(answer);
        }

        set_replication_set(&emulator, replication_set.clone());

        for row in &rows {
            let case = format!("{account_case}: {}", row.case);
            let (status, body) = emulator.call(row.operation, row.request.clone()).await;

            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400 for an account with no active replication \
                 set, got {status} with {body:?}"
            );
            assert!(
                error_code(&body).contains("ValidationException"),
                "case {case:?}: expected the error code ValidationException — NOT a \
                 ResourceNotFoundException, even where the ARN resolves to nothing — got \
                 {:?} in {body:?}",
                error_code(&body)
            );

            // The refusal reports the ACCOUNT-level precondition: it names the account or
            // the ARN that cannot be resolved, and blames no other member of the request —
            // the remedy for the two is different.
            let message = body["Message"].as_str().unwrap_or_default();
            assert!(
                message.contains("Account") || message.contains(row.subject.as_str()),
                "case {case:?}: expected a Message reporting the account or the supplied \
                 ARN {:?}, got {message:?} in {body:?}",
                row.subject
            );
            for member in ["NextToken", "nextToken", "MaxResults"] {
                assert!(
                    !message.contains(member),
                    "case {case:?}: an account-level refusal must not blame {member}, got \
                     {message:?} in {body:?}"
                );
            }

            // No member of the request is at fault, so `Reason` stays inside the modelled
            // `ValidationExceptionReason` set (the crate's own model-derived list, imported
            // rather than copied) and `Fields` is present but empty.
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

        // A read is a read: once the account is onboarded again every row answers exactly
        // what it answered before the refusals.
        onboard(&emulator);
        for (row, want) in rows.iter().zip(before) {
            let case = format!("{account_case}: {} (onboarded again)", row.case);
            let answer = emulator.call(row.operation, row.request.clone()).await;
            assert_eq!(
                answer, want,
                "case {case:?}: a refused read must change nothing a later caller sees, got \
                 {answer:?}"
            );
        }
    }
}

/// The control: `ListContacts` resolves no resource ARN, so it is served whatever the
/// account's replication set is — which is what makes the 400s above a statement about the
/// gate rather than about the service being unreachable.
#[tokio::test]
async fn an_account_wide_listing_is_served_without_a_replication_set() {
    let emulator = Emulator::new();
    let fixture = fixture(&emulator).await;

    for (account_case, replication_set) in [
        ("no replication set at all", None),
        (
            "a replication set that has not finished replicating",
            Some(AccountReplicationSet {
                active: false,
                ..active_replication_set()
            }),
        ),
    ] {
        set_replication_set(&emulator, replication_set);
        let (status, body) = emulator
            .call("ListContacts", json!({"AliasPrefix": "oncall"}))
            .await;
        assert_eq!(
            status, 200,
            "case {account_case:?}: expected ListContacts to stay served, got {status} \
             with {body:?}"
        );
        let aliases: Vec<&str> = body["Contacts"]
            .as_array()
            .map(|contacts| {
                contacts
                    .iter()
                    .filter_map(|contact| contact["Alias"].as_str())
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            aliases,
            vec!["oncall-primary", "oncall-secondary"],
            "case {account_case:?}: expected the stored contacts to be reported, got {body:?}"
        );
    }

    // And the gated reads of the very same resources are refused on that same account.
    for (operation, request) in [
        ("GetContact", json!({"ContactId": fixture.busy_contact})),
        (
            "GetContactChannel",
            json!({"ContactChannelId": fixture.channel}),
        ),
        (
            "ListContactChannels",
            json!({"ContactId": fixture.busy_contact}),
        ),
    ] {
        let (status, body) = emulator.call(operation, request).await;
        assert_eq!(
            status, 400,
            "case {operation:?}: expected the account gate to refuse the ARN-addressed read \
             on the same account ListContacts is served on, got {status} with {body:?}"
        );
    }
}
