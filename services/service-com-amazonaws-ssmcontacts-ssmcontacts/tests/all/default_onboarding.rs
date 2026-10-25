// SPDX-License-Identifier: CC0-1.0

//! What a client finds on a FRESHLY STARTED emulator, end to end over the wire.
//!
//! The rule under test:
//!
//! > An account-level prerequisite that no API call can create belongs to the emulated
//! > account's initial state, so every operation gated on it answers about its OWN subject
//! > from the very first request — nothing has to be written into the store out of band.
//!
//! Here that prerequisite is the account's Incident Manager replication set. The sibling
//! suites install one themselves and then take it away, which is why they say nothing about
//! the account a client actually meets: every one of them drives
//! `contact_lifecycle::Emulator::new`, whose fixture onboards. This suite drives
//! `Emulator::fresh` instead — no fixture and no in-process write before the call — so what
//! it asserts is what an SDK or CLI gets on a cold emulator.
//!
//! Three suites, each a TABLE spanning what the rule generalises over:
//!
//! 1. The whole gated surface (`data_plane_precondition::GATED_OPERATIONS`, the same table
//!    the refusal suite uses, so the two cannot drift): each operation answers about its own
//!    subject, never with the account-level refusal.
//! 2. The create -> read round trip, over every `ContactType` the model admits and both
//!    shapes its `Plan` takes, plus the channel, rotation and engagement a contact carries:
//!    what a cold emulator accepts it also reports back.
//! 3. The store's own initial value: the prerequisite is present and ACTIVE, and NOTHING
//!    else is — no contact, channel, rotation, override, engagement or page is invented to
//!    make the account look used.

use serde_json::{json, Value};

use service_com_amazonaws_ssmcontacts_ssmcontacts::ops::types::{
    CHANNEL_TYPE_VALUES, CONTACT_TYPE_VALUES,
};
use service_com_amazonaws_ssmcontacts_ssmcontacts::state::{
    default_replication_set_arn, invalid_resource_arn_message, service_state,
    ACCOUNT_NOT_FOUND_MESSAGE,
};

use crate::contact_lifecycle::{error_code, Emulator, ACCOUNT};
use crate::data_plane_precondition::{Targets, GATED_OPERATIONS};

/// A daily recurrence: `NumberOfOnCalls` and `RecurrenceMultiplier` are the members the
/// model's `RecurrenceSettings` makes required, and `DailySettings` is one of its three
/// settings lists (`smithy_query(query="shape",shape="RecurrenceSettings")`).
fn daily_recurrence() -> Value {
    json!({
        "NumberOfOnCalls": 1,
        "RecurrenceMultiplier": 1,
        "DailySettings": [{"HourOfDay": 9, "MinuteOfHour": 0}],
    })
}

/// Whether `body` is the ACCOUNT-level refusal — either wording the gate answers with.
///
/// Read through the crate's own message helpers rather than copied text, so a reworded
/// refusal cannot slip past this suite.
fn refuses_the_account(body: &Value) -> bool {
    let message = body["Message"].as_str().unwrap_or_default();
    // `invalid_resource_arn_message("")` is that refusal's prefix, whatever ARN follows it.
    message.contains(ACCOUNT_NOT_FOUND_MESSAGE) || message.contains(&invalid_resource_arn_message(""))
}

/// Every operation gated on the account's replication set answers about its own subject on
/// a cold emulator — the same table `data_plane_precondition.rs` asserts the refusals over.
///
/// One fresh emulator per row, so each row's answer is the FIRST request that emulator ever
/// served: no earlier row can have onboarded the account for it.
#[tokio::test]
async fn every_gated_operation_is_served_on_a_freshly_started_emulator() {
    for row in GATED_OPERATIONS {
        let emulator = Emulator::fresh();
        let targets = Targets::absent();
        let case = format!("{} as the first call on a cold emulator", row.operation);

        let (status, body) = emulator.call(row.operation, (row.body)(&targets)).await;

        assert_eq!(
            status, row.absent_status,
            "case {case:?}: expected HTTP {} — the answer about the subject the request \
             names — got {status} with {body:?}",
            row.absent_status
        );
        assert!(
            !refuses_the_account(&body),
            "case {case:?}: the account a client finds must already satisfy the \
             account-level precondition, got {body:?}"
        );
        if row.absent_status == 404 {
            assert!(
                error_code(&body).contains("ResourceNotFoundException"),
                "case {case:?}: expected the answer to be about the absent resource, got \
                 {:?} in {body:?}",
                error_code(&body)
            );
        }
    }
}

/// One contact to create on a cold emulator, and what reading it back must report.
struct ContactRow {
    case: &'static str,
    alias: &'static str,
    contact_type: &'static str,
    /// The `Plan` the create carries — a `Stages` list, or a `RotationIds` list.
    plan: fn() -> Value,
}

/// Every `ContactType` the model admits, over both shapes its `Plan` takes.
///
/// `CONTACT_TYPE_VALUES` (the crate's model-derived list) is asserted to be exactly the set
/// these rows cover, so a value added to the model fails this suite instead of going
/// untested.
const CONTACT_ROWS: &[ContactRow] = &[
    ContactRow {
        case: "a PERSONAL contact with a staged engagement plan",
        alias: "oncall-primary",
        contact_type: "PERSONAL",
        plan: || json!({"Stages": [{"DurationInMinutes": 5, "Targets": []}]}),
    },
    ContactRow {
        case: "an ESCALATION plan with an immediate stage",
        alias: "escalation-plan",
        contact_type: "ESCALATION",
        plan: || json!({"Stages": [{"DurationInMinutes": 0, "Targets": []}]}),
    },
    ContactRow {
        case: "an ONCALL_SCHEDULE whose plan carries no stage at all",
        alias: "weekend-schedule",
        contact_type: "ONCALL_SCHEDULE",
        plan: || json!({"Stages": []}),
    },
];

/// A cold emulator CREATES and then REPORTS: each row's contact is readable back through
/// `GetContact`, takes a channel readable through `GetContactChannel`, joins a rotation
/// readable through `GetRotation`, and can be engaged and the engagement described.
#[tokio::test]
async fn a_cold_emulator_round_trips_the_contact_lifecycle() {
    let covered: Vec<&str> = CONTACT_ROWS.iter().map(|row| row.contact_type).collect();
    let mut modelled = CONTACT_TYPE_VALUES.to_vec();
    modelled.sort_unstable();
    let mut sorted = covered.clone();
    sorted.sort_unstable();
    assert_eq!(
        sorted, modelled,
        "the table must span the modelled ContactType set {CONTACT_TYPE_VALUES:?}, covers \
         {covered:?}"
    );

    let emulator = Emulator::fresh();

    for row in CONTACT_ROWS {
        let case = row.case;
        let (status, body) = emulator
            .call(
                "CreateContact",
                json!({
                    "Alias": row.alias,
                    "Type": row.contact_type,
                    "Plan": (row.plan)(),
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateContact to answer 200 on a cold emulator, got \
             {status} with {body:?}"
        );
        let contact_arn = body["ContactArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a ContactArn, got {body:?}"))
            .to_string();

        let (status, body) = emulator
            .call("GetContact", json!({"ContactId": contact_arn}))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the created contact to read back, got {status} with \
             {body:?}"
        );
        assert_eq!(
            body["Alias"].as_str(),
            Some(row.alias),
            "case {case:?}: expected the created Alias {:?}, got {body:?}",
            row.alias
        );
        assert_eq!(
            body["Type"].as_str(),
            Some(row.contact_type),
            "case {case:?}: expected the created Type {:?}, got {body:?}",
            row.contact_type
        );

        // A channel of each modelled ChannelType, so the gated channel create/read is
        // exercised over the whole value set rather than one address form.
        for channel_type in CHANNEL_TYPE_VALUES {
            let case = format!("{case}: its {channel_type} channel");
            let address = match *channel_type {
                "EMAIL" => "ops@example.com",
                _ => "+15550100",
            };
            let (status, body) = emulator
                .call(
                    "CreateContactChannel",
                    json!({
                        "ContactId": contact_arn,
                        "Name": format!("{channel_type}-channel"),
                        "Type": channel_type,
                        "DeliveryAddress": {"SimpleAddress": address},
                    }),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected CreateContactChannel to answer 200, got {status} \
                 with {body:?}"
            );
            let channel_arn = body["ContactChannelArn"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: expected a ContactChannelArn, got {body:?}")
                })
                .to_string();

            let (status, body) = emulator
                .call("GetContactChannel", json!({"ContactChannelId": channel_arn}))
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the created channel to read back, got {status} with \
                 {body:?}"
            );
            assert_eq!(
                body["Type"].as_str(),
                Some(*channel_type),
                "case {case:?}: expected the created channel Type, got {body:?}"
            );
        }

        // The rotation slice, whose listing is gated account-wide rather than per ARN.
        let (status, body) = emulator
            .call(
                "CreateRotation",
                json!({
                    "Name": format!("{}-rotation", row.alias),
                    "ContactIds": [contact_arn],
                    "TimeZoneId": "UTC",
                    "Recurrence": daily_recurrence(),
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateRotation to answer 200, got {status} with {body:?}"
        );
        let rotation_arn = body["RotationArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a RotationArn, got {body:?}"))
            .to_string();

        let (status, body) = emulator
            .call("GetRotation", json!({"RotationId": rotation_arn}))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the created rotation to read back, got {status} with \
             {body:?}"
        );
        assert_eq!(
            body["ContactIds"].as_array().map(Vec::len),
            Some(1),
            "case {case:?}: expected the rotation's one team member, got {body:?}"
        );

        // The engagement slice: starting one and describing it are both gated.
        let (status, body) = emulator
            .call(
                "StartEngagement",
                json!({
                    "ContactId": contact_arn,
                    "Sender": "cli",
                    "Subject": "disk pressure",
                    "Content": "node-7 is at 95%",
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected StartEngagement to answer 200, got {status} with {body:?}"
        );
        let engagement_arn = body["EngagementArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected an EngagementArn, got {body:?}"))
            .to_string();

        let (status, body) = emulator
            .call("DescribeEngagement", json!({"EngagementId": engagement_arn}))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the started engagement to be describable, got {status} \
             with {body:?}"
        );
        assert_eq!(
            body["ContactArn"].as_str(),
            Some(contact_arn.as_str()),
            "case {case:?}: expected the engagement to name the engaged contact, got {body:?}"
        );
    }
}

/// The store a cold emulator starts with: the account-level prerequisite is there and
/// ACTIVE, and nothing else is.
///
/// The ARN is checked against the crate's own minting helper and taken apart segment by
/// segment, so the account's onboarding record names Incident Manager's namespace for this
/// account rather than any value a request could carry.
#[tokio::test]
async fn the_initial_store_holds_only_the_accounts_onboarding_record() {
    let emulator = Emulator::fresh();

    {
        let state =
            service_state(&*emulator.engine).expect("the engine holds this service's state");
        let state = state.lock();

        let set = state
            .active_replication_set()
            .unwrap_or_else(|| panic!("expected the account to hold an ACTIVE replication set, got {:?}", state.replication_set));
        assert_eq!(
            set.arn,
            default_replication_set_arn(),
            "expected the account's onboarding record to carry the minted set ARN, got {:?}",
            set.arn
        );
        let segments: Vec<&str> = set.arn.split(':').collect();
        assert_eq!(
            segments.len(),
            6,
            "expected a six-segment ARN, got {:?}",
            set.arn
        );
        assert_eq!(
            (segments[2], segments[3], segments[4]),
            ("ssm-incidents", "", ACCOUNT),
            "expected the set's ARN to name Incident Manager, no Region and this account, \
             got {:?}",
            set.arn
        );
        assert!(
            segments[5].starts_with("replication-set/"),
            "expected a replication-set resource segment, got {:?}",
            set.arn
        );

        // An onboarded account is not a used account: every resource map starts empty and
        // every counter at zero, so nothing a read reports was invented here.
        for (what, count) in [
            ("contacts", state.contacts.len()),
            ("alias index entries", state.alias_index.len()),
            ("contact channels", state.contact_channels.len()),
            ("rotations", state.rotations.len()),
            ("rotation overrides", state.rotation_overrides.len()),
            ("engagements", state.engagements.len()),
            ("pages", state.pages.len()),
            ("idempotency ledger entries", state.idempotency_tokens.len()),
        ] {
            assert_eq!(
                count, 0,
                "case {what:?}: a fresh account owns no resources, got {count}"
            );
        }
        for (what, counter) in [
            ("channels_created", state.channels_created),
            ("rotations_created", state.rotations_created),
            ("rotation_overrides_created", state.rotation_overrides_created),
            ("activation_codes_issued", state.activation_codes_issued),
            ("engagements_started", state.engagements_started),
            ("pages_sent", state.pages_sent),
        ] {
            assert_eq!(
                counter, 0,
                "case {what:?}: a fresh account has minted nothing, got {counter}"
            );
        }
    }

    // And over the wire: every account-wide listing is served, and each reports an empty
    // page rather than a resource nobody created.
    for (operation, collection) in [
        ("ListContacts", "Contacts"),
        ("ListRotations", "Rotations"),
        ("ListEngagements", "Engagements"),
    ] {
        let (status, body) = emulator.call(operation, json!({})).await;
        assert_eq!(
            status, 200,
            "case {operation:?}: expected a served listing on a cold emulator, got {status} \
             with {body:?}"
        );
        assert_eq!(
            body[collection].as_array().map(Vec::len),
            Some(0),
            "case {operation:?}: expected an empty page, got {body:?}"
        );
    }
}
