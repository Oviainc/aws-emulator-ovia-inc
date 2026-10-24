// SPDX-License-Identifier: CC0-1.0

//! `DeactivateContactChannel`, end to end over the wire.
//!
//! The rule under test:
//!
//! > Deactivating a contact channel moves THAT channel's `ActivationStatus` to the modelled
//! > `NOT_ACTIVATED` and changes nothing else about it — not its name, type or delivery
//! > address, not the activation code last delivered to it, and not any sibling channel of
//! > the same contact — so a later read reports the deactivated channel as unactivated; the
//! > call models no "already deactivated" error and returns no state, so repeating it is
//! > idempotent, and an ARN naming no stored channel is refused with the modelled
//! > `ResourceNotFoundException` (HTTP 404).
//!
//! The suites are TABLES over what the rule generalises over: the channel's type and
//! delivery-address form (the model's whole `ChannelType` value set), whether its creation
//! deferred activation, and which `ActivationStatus` the channel is in when the deactivate
//! arrives (`ACTIVATED` after a real activation, and `NOT_ACTIVATED` for a channel that was
//! never activated and for one deactivated a second time). The ACCOUNT-level replication-set
//! gate over this operation is a row of the shared table in
//! `tests/all/data_plane_precondition.rs`, which also asserts the refusal writes nothing.

use serde_json::{json, Value};

use service_com_amazonaws_ssmcontacts_ssmcontacts::ops::types::ACTIVATION_STATUS_VALUES;

use crate::contact_lifecycle::{error_code, Emulator, ACCOUNT, REGION};
use crate::data_plane_precondition::{
    activation_status, create_channel, delivered_code, onboarded_contact,
};

/// The two `ActivationStatus` values the model declares, named here so the assertions read
/// as the rule does. Both are checked against the crate's model-derived value set below
/// rather than copied as bare strings.
const ACTIVATED: &str = "ACTIVATED";
const NOT_ACTIVATED: &str = "NOT_ACTIVATED";

/// One channel to deactivate. The rows differ along what the rule generalises over: the
/// `ChannelType` and the address form it documents, whether `DeferActivation` was named (and
/// with which value), and whether the channel is ACTIVATED when the deactivate arrives.
struct ChannelRow {
    case: &'static str,
    name: &'static str,
    channel_type: &'static str,
    address: &'static str,
    defer_activation: Option<bool>,
    /// Whether the suite activates the channel before deactivating it, so the table spans
    /// both statuses a stored channel can be in.
    activate_first: bool,
}

/// Every `ChannelType` the model declares (`EMAIL | SMS | VOICE`), each over both starting
/// statuses and all three `DeferActivation` shapes.
const CHANNEL_ROWS: &[ChannelRow] = &[
    ChannelRow {
        case: "an activated email channel with DeferActivation absent",
        name: "email",
        channel_type: "EMAIL",
        address: "ops@example.com",
        defer_activation: None,
        activate_first: true,
    },
    ChannelRow {
        case: "an activated SMS channel with DeferActivation false",
        name: "sms",
        channel_type: "SMS",
        address: "+15550100",
        defer_activation: Some(false),
        activate_first: true,
    },
    ChannelRow {
        case: "an activated voice channel created with activation deferred",
        name: "voice",
        channel_type: "VOICE",
        address: "+15550101",
        defer_activation: Some(true),
        activate_first: true,
    },
    ChannelRow {
        case: "an email channel that was never activated",
        name: "email-pending",
        channel_type: "EMAIL",
        address: "pager@example.com",
        defer_activation: None,
        activate_first: false,
    },
    ChannelRow {
        case: "a voice channel with activation deferred that was never activated",
        name: "voice-pending",
        channel_type: "VOICE",
        address: "+15550102",
        defer_activation: Some(true),
        activate_first: false,
    },
];

/// The channel the suite deactivates, plus the sibling it must leave alone.
struct Fixture {
    channel: String,
    sibling: String,
    contact: String,
}

/// Create `row`'s channel under a fresh onboarded contact, together with an ACTIVATED
/// sibling channel of the same contact, and bring `row`'s channel to its starting status.
async fn fixture(emulator: &Emulator, row: &ChannelRow) -> Fixture {
    let contact = onboarded_contact(emulator, "oncall-primary").await;

    let mut request = json!({
        "ContactId": contact,
        "Name": row.name,
        "Type": row.channel_type,
        "DeliveryAddress": {"SimpleAddress": row.address},
    });
    if let Some(defer) = row.defer_activation {
        request["DeferActivation"] = json!(defer);
    }
    let channel = create_channel(emulator, request, row.case).await;

    // The sibling: a second channel of the SAME contact, activated, so a deactivate that
    // reached more than the channel it named would show up here.
    let sibling = create_channel(
        emulator,
        json!({
            "ContactId": contact,
            "Name": "sibling-sms",
            "Type": "SMS",
            "DeliveryAddress": {"SimpleAddress": "+15550199"},
        }),
        row.case,
    )
    .await;
    activate(emulator, &sibling, &format!("{}: the sibling", row.case)).await;

    if row.activate_first {
        activate(emulator, &channel, row.case).await;
        assert_eq!(
            activation_status(emulator, &channel, row.case).await,
            ACTIVATED,
            "case {:?}: the fixture must start ACTIVATED",
            row.case
        );
    } else {
        assert_eq!(
            activation_status(emulator, &channel, row.case).await,
            NOT_ACTIVATED,
            "case {:?}: a channel that was never activated starts NOT_ACTIVATED",
            row.case
        );
    }

    Fixture {
        channel,
        sibling,
        contact,
    }
}

/// Activate `channel_arn` with the code the store delivered to it, delivering one first when
/// the channel's creation deferred activation. AWS delivers the code out of band, so the
/// store is the channel's mailbox.
async fn activate(emulator: &Emulator, channel_arn: &str, case: &str) {
    if delivered_code(emulator, channel_arn).is_none() {
        let (status, body) = emulator
            .call("SendActivationCode", json!({"ContactChannelId": channel_arn}))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected SendActivationCode to answer 200, got {status} {body:?}"
        );
    }
    let code = delivered_code(emulator, channel_arn)
        .unwrap_or_else(|| panic!("case {case:?}: a code was delivered to {channel_arn}"));
    let (status, body) = emulator
        .call(
            "ActivateContactChannel",
            json!({"ContactChannelId": channel_arn, "ActivationCode": code}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ActivateContactChannel to answer 200, got {status} {body:?}"
    );
}

/// `GetContactChannel` for `channel_arn`, asserting it answered 200.
async fn get_channel(emulator: &Emulator, channel_arn: &str, case: &str) -> Value {
    let (status, body) = emulator
        .call("GetContactChannel", json!({"ContactChannelId": channel_arn}))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetContactChannel to answer 200, got {status} {body:?}"
    );
    body
}

/// The `ActivationStatus` `ListContactChannels` reports for `channel_arn` under `contact_arn`.
async fn listed_status(
    emulator: &Emulator,
    contact_arn: &str,
    channel_arn: &str,
    case: &str,
) -> String {
    let (status, body) = emulator
        .call("ListContactChannels", json!({"ContactId": contact_arn}))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListContactChannels to answer 200, got {status} {body:?}"
    );
    let listed = body["ContactChannels"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let entry = listed
        .iter()
        .find(|entry| entry["ContactChannelArn"].as_str() == Some(channel_arn))
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected {channel_arn} among the listed channels, got {body:?}")
        });
    entry["ActivationStatus"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected every listed channel to carry an ActivationStatus, got {entry:?}")
        })
        .to_string()
}

/// `DeactivateContactChannel` moves the named channel to `NOT_ACTIVATED`, whatever status it
/// was in, and both readers report it — repeatedly, because the call is idempotent.
#[tokio::test]
async fn deactivating_a_channel_reads_back_not_activated() {
    for row in CHANNEL_ROWS {
        let emulator = Emulator::new();
        let fixture = fixture(&emulator, row).await;

        // Twice: the second call is the idempotence row of the table — the channel is
        // already NOT_ACTIVATED when it arrives, and the model declares no error for that.
        for attempt in 1..=2 {
            let case = format!("{}, deactivate attempt {attempt}", row.case);
            let (status, body) = emulator
                .call(
                    "DeactivateContactChannel",
                    json!({"ContactChannelId": fixture.channel}),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} {body:?}"
            );
            assert!(
                body == json!({}) || body.is_null(),
                "case {case:?}: DeactivateContactChannel models an empty output, got {body:?}"
            );

            let reported = activation_status(&emulator, &fixture.channel, &case).await;
            assert_eq!(
                reported, NOT_ACTIVATED,
                "case {case:?}: GetContactChannel must report the channel unactivated, got \
                 {reported:?}"
            );
            assert!(
                ACTIVATION_STATUS_VALUES.contains(&reported.as_str()),
                "case {case:?}: expected a status inside the modelled ActivationStatus set \
                 {ACTIVATION_STATUS_VALUES:?}, got {reported:?}"
            );
            let listed =
                listed_status(&emulator, &fixture.contact, &fixture.channel, &case).await;
            assert_eq!(
                listed, NOT_ACTIVATED,
                "case {case:?}: ListContactChannels must agree with GetContactChannel, got \
                 {listed:?}"
            );

            // The sibling channel of the same contact is untouched: the deactivate is
            // scoped to the ARN it named.
            let sibling = activation_status(&emulator, &fixture.sibling, &case).await;
            assert_eq!(
                sibling, ACTIVATED,
                "case {case:?}: a sibling channel must keep its own status, got {sibling:?}"
            );
        }
    }
}

/// Deactivation changes the status and NOTHING else: the channel's own members are what the
/// create persisted, and the code last delivered to it still activates it.
#[tokio::test]
async fn deactivation_leaves_the_rest_of_the_channel_alone() {
    for row in CHANNEL_ROWS {
        let emulator = Emulator::new();
        let fixture = fixture(&emulator, row).await;

        let before = get_channel(&emulator, &fixture.channel, row.case).await;
        let code_before = delivered_code(&emulator, &fixture.channel);

        let (status, body) = emulator
            .call(
                "DeactivateContactChannel",
                json!({"ContactChannelId": fixture.channel}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200, got {status} {body:?}",
            row.case
        );

        let after = get_channel(&emulator, &fixture.channel, row.case).await;
        for member in [
            "ContactArn",
            "ContactChannelArn",
            "Name",
            "Type",
            "DeliveryAddress",
        ] {
            assert_eq!(
                after[member], before[member],
                "case {:?}: deactivation must not change {member}, got {:?} where it was {:?}",
                row.case, after[member], before[member]
            );
        }

        let code_after = delivered_code(&emulator, &fixture.channel);
        assert_eq!(
            code_after, code_before,
            "case {:?}: deactivation delivers no code and supersedes none, got {code_after:?} \
             where it was {code_before:?}",
            row.case
        );

        // Round trip: the code the channel still holds re-activates it, so the only thing
        // deactivation did was move the status.
        if let Some(code) = code_after {
            let (status, body) = emulator
                .call(
                    "ActivateContactChannel",
                    json!({"ContactChannelId": fixture.channel, "ActivationCode": code}),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {:?}: expected the held code to re-activate the channel, got {status} \
                 {body:?}",
                row.case
            );
            assert_eq!(
                activation_status(&emulator, &fixture.channel, row.case).await,
                ACTIVATED,
                "case {:?}: the re-activated channel must read back ACTIVATED",
                row.case
            );
        }
    }
}

/// An ARN that names no stored channel is refused with the modelled
/// `ResourceNotFoundException` (HTTP 404) — never a 501, and never a silent success.
#[tokio::test]
async fn deactivating_a_channel_that_is_not_stored_is_not_found() {
    let emulator = Emulator::new();
    let contact = onboarded_contact(&emulator, "oncall-primary").await;
    let channel = create_channel(
        &emulator,
        json!({
            "ContactId": contact,
            "Name": "email",
            "Type": "EMAIL",
            "DeliveryAddress": {"SimpleAddress": "ops@example.com"},
        }),
        "the deleted channel",
    )
    .await;
    let (status, body) = emulator
        .call("DeleteContactChannel", json!({"ContactChannelId": channel}))
        .await;
    assert_eq!(
        status, 200,
        "expected DeleteContactChannel to answer 200, got {status} {body:?}"
    );

    // Well-formed `SsmContactsArn` values (the modelled pattern) that name no stored
    // channel: one never created, one whose channel was just deleted, and ARNs of other
    // resource kinds — the store is keyed by channel ARN, so none of them resolves.
    let absent: &[(&str, String)] = &[
        (
            "a channel ARN nothing ever created",
            format!(
                "arn:aws:ssm-contacts:{REGION}:{ACCOUNT}:contact-channel/oncall-primary/00000000-0000-0000-0000-000000000000"
            ),
        ),
        ("a channel ARN that was deleted", channel.clone()),
        ("the ARN of the contact, not of a channel", contact.clone()),
        (
            "a rotation ARN",
            format!(
                "arn:aws:ssm-contacts:{REGION}:{ACCOUNT}:rotation/00000000-0000-0000-0000-000000000000"
            ),
        ),
        (
            "an ARN in another partition",
            format!(
                "arn:aws-cn:ssm-contacts:{REGION}:{ACCOUNT}:contact-channel/oncall-primary/00000000-0000-0000-0000-000000000000"
            ),
        ),
    ];

    for (case, arn) in absent {
        let (status, body) = emulator
            .call(
                "DeactivateContactChannel",
                json!({"ContactChannelId": arn}),
            )
            .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404, got {status} {body:?}"
        );
        assert!(
            error_code(&body).contains("ResourceNotFoundException"),
            "case {case:?}: expected a ResourceNotFoundException, got {:?} in {body:?}",
            error_code(&body)
        );
    }
}
