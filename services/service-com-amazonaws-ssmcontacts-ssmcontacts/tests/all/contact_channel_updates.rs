// SPDX-License-Identifier: CC0-1.0

//! `UpdateContactChannel`, end to end over the wire.
//!
//! The rule under test:
//!
//! > An update of a contact channel is a PATCH on the stored channel: each member the
//! > request names (`Name`, `DeliveryAddress`) replaces the stored one, each member it omits
//! > is left exactly as it was, and nothing else moves — not the channel's ARN, its
//! > `ContactArn` or its `Type`. A channel's `Name` stays unique within its contact, so a
//! > rename onto a sibling's name is refused with `ConflictException` (HTTP 409); an ARN
//! > naming no stored channel is `ResourceNotFoundException` (HTTP 404); a request naming
//! > neither updatable member, or a `DeliveryAddress` carrying no `SimpleAddress`, is
//! > `ValidationException` (HTTP 400); and every refusal writes nothing. Because an
//! > `ACTIVATED` status attests the address the activation code was delivered to, a change
//! > of `DeliveryAddress` returns the channel to `NOT_ACTIVATED` and supersedes the
//! > outstanding code, while a rename leaves both alone.
//!
//! Every suite is a TABLE over what the rule generalises over — the channel's `Type` and
//! address form (the model's whole `ChannelType` value set), whether its creation deferred
//! activation and whether it is `ACTIVATED` when the update arrives, which members the
//! request names (one, the other, both), and the value a named member carries (a rename, the
//! name it already had, `ChannelName`'s published bounds 1 and 255, a non-ASCII name,
//! `SimpleAddress`' published bounds 1 and 320, the address it already had). Each row's
//! expectation is COMPUTED as "what was created, overlaid with what the request named", so
//! the assertions state the rule rather than a transcript.
//!
//! The service does not tie a channel's address FORM to its `Type` — `CreateContactChannel`
//! stores whatever `SimpleAddress` the request carries — so the address rows are applied to
//! every channel type.

use serde_json::{json, Value};

use service_com_amazonaws_ssmcontacts_ssmcontacts::ops::types::ACTIVATION_STATUS_VALUES;
use service_com_amazonaws_ssmcontacts_ssmcontacts::state::{service_state, AccountReplicationSet};

use crate::contact_lifecycle::{error_code, Emulator, ACCOUNT, REGION};
use crate::contact_updates::{active_replication_set, onboard, set_replication_set};
use crate::data_plane_precondition::{create_channel, delivered_code, onboarded_contact};

/// The two `ActivationStatus` values the model declares, named so the assertions read as the
/// rule does. Both are checked against the crate's model-derived value set below.
const ACTIVATED: &str = "ACTIVATED";
const NOT_ACTIVATED: &str = "NOT_ACTIVATED";

// ---------------------------------------------------------------------------
// The tables
// ---------------------------------------------------------------------------

/// The channel an update is applied to, as `CreateContactChannel` made it.
///
/// The rows span the model's whole `ChannelType` value set (`EMAIL | SMS | VOICE`) with the
/// address form each one documents, all three `DeferActivation` shapes, and both
/// `ActivationStatus` values a stored channel can be in when the update arrives.
struct BaseChannel {
    case: &'static str,
    name: &'static str,
    channel_type: &'static str,
    address: &'static str,
    defer_activation: Option<bool>,
    /// Whether the suite activates the channel before updating it.
    activate_first: bool,
}

const BASE_CHANNELS: &[BaseChannel] = &[
    BaseChannel {
        case: "an activated email channel with DeferActivation absent",
        name: "email",
        channel_type: "EMAIL",
        address: "ops@example.com",
        defer_activation: None,
        activate_first: true,
    },
    BaseChannel {
        case: "an SMS channel with DeferActivation false that was never activated",
        name: "mobile sms.work-1",
        channel_type: "SMS",
        address: "+15550100",
        defer_activation: Some(false),
        activate_first: false,
    },
    BaseChannel {
        case: "an activated voice channel created with activation deferred",
        name: "_4155550101",
        channel_type: "VOICE",
        address: "+15550101",
        defer_activation: Some(true),
        activate_first: true,
    },
];

/// One update to apply: the members the `UpdateContactChannel` request names beside
/// `ContactChannelId`.
///
/// The rows differ along what the PATCH rule generalises over: which optional members the
/// request names (`Name` only, `DeliveryAddress` only, both), and the value a named one
/// carries. The name values span the modelled `ChannelName` shape — len 1..255, pattern
/// `^[\p{L}\p{Z}\p{N}_.\-]*$` (`smithy_query(query="shape",shape="ChannelName")`) — and the
/// address values span the modelled `SimpleAddress` length 1..320
/// (`smithy_query(query="shape",shape="ContactChannelAddress")`). Two rows deliberately name
/// the value the channel ALREADY carries, which is a write that changes nothing.
struct UpdateRow {
    case: &'static str,
    /// Built from the channel's CURRENT name and address, so a row can name the value the
    /// channel already carries however many updates it has already taken.
    patch: fn(&str, &str) -> Value,
}

const UPDATE_ROWS: &[UpdateRow] = &[
    UpdateRow {
        case: "Name only, a plain rename",
        patch: |_, _| json!({"Name": "renamed channel"}),
    },
    UpdateRow {
        case: "Name only, at ChannelName's published minimum length of 1",
        patch: |_, _| json!({"Name": "e"}),
    },
    UpdateRow {
        case: "Name only, at ChannelName's published maximum length of 255",
        patch: |_, _| json!({"Name": "n".repeat(255)}),
    },
    UpdateRow {
        case: "Name only, non-ASCII letters with a separator, digits, '_', '.' and '-'",
        patch: |_, _| json!({"Name": "Überwachung téléphone_2.work-1"}),
    },
    UpdateRow {
        case: "Name only, the name the channel already carries",
        patch: |name, _| json!({"Name": name}),
    },
    UpdateRow {
        case: "DeliveryAddress only, a different address",
        patch: |_, _| json!({"DeliveryAddress": {"SimpleAddress": "oncall@example.com"}}),
    },
    UpdateRow {
        case: "DeliveryAddress only, at SimpleAddress' published minimum length of 1",
        patch: |_, _| json!({"DeliveryAddress": {"SimpleAddress": "x"}}),
    },
    UpdateRow {
        case: "DeliveryAddress only, at SimpleAddress' published maximum length of 320",
        patch: |_, _| {
            json!({"DeliveryAddress": {"SimpleAddress": long_address()}})
        },
    },
    UpdateRow {
        case: "DeliveryAddress only, the address the channel already carries",
        patch: |_, address| json!({"DeliveryAddress": {"SimpleAddress": address}}),
    },
    UpdateRow {
        case: "both members named",
        patch: |_, _| {
            json!({
                "Name": "paging-escalation.1",
                "DeliveryAddress": {"SimpleAddress": "+15550199"},
            })
        },
    },
    UpdateRow {
        case: "both members named, the address being the one already stored",
        patch: |_, address| {
            json!({
                "Name": "kept-address",
                "DeliveryAddress": {"SimpleAddress": address},
            })
        },
    },
];

/// An address exactly at `SimpleAddress`' published maximum length of 320.
fn long_address() -> String {
    let domain = "@example.com";
    format!("{}{domain}", "a".repeat(320 - domain.len()))
}

/// What a read must report for a channel that carried `name_before`/`address_before` in
/// `status_before` after `patch` was applied: the stored channel overlaid with the members
/// the request named. This IS the rule under test.
fn expected_after(
    name_before: &str,
    address_before: &str,
    patch: &Value,
    status_before: &str,
) -> (String, String, String) {
    let name = patch
        .get("Name")
        .and_then(Value::as_str)
        .unwrap_or(name_before)
        .to_string();
    let named_address = patch
        .get("DeliveryAddress")
        .and_then(|address| address.get("SimpleAddress"))
        .and_then(Value::as_str);
    let address = named_address.unwrap_or(address_before).to_string();
    // The activation attests the address the code went to, so only a CHANGE of address
    // returns the channel to NOT_ACTIVATED.
    let status = if named_address.is_some_and(|named| named != address_before) {
        NOT_ACTIVATED.to_string()
    } else {
        status_before.to_string()
    };
    (name, address, status)
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// The `CreateContactChannel` body for `base` under `contact_arn`.
fn create_request(contact_arn: &str, base: &BaseChannel) -> Value {
    let mut request = json!({
        "ContactId": contact_arn,
        "Name": base.name,
        "Type": base.channel_type,
        "DeliveryAddress": {"SimpleAddress": base.address},
    });
    if let Some(defer) = base.defer_activation {
        request["DeferActivation"] = json!(defer);
    }
    request
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

/// A contact with one channel built from `base`, in the `ActivationStatus` the row asks for.
/// Answers the contact ARN, the channel ARN and the status the update will find it in.
async fn base_channel(
    emulator: &Emulator,
    alias: &str,
    base: &BaseChannel,
) -> (String, String, String) {
    let contact = onboarded_contact(emulator, alias).await;
    let channel = create_channel(emulator, create_request(&contact, base), base.case).await;
    if base.activate_first {
        activate(emulator, &channel, base.case).await;
        (contact, channel, ACTIVATED.to_string())
    } else {
        (contact, channel, NOT_ACTIVATED.to_string())
    }
}

/// `UpdateContactChannel` for `channel_arn` with `patch`.
async fn update(emulator: &Emulator, channel_arn: &str, patch: &Value) -> (u16, Value) {
    let mut request = patch.clone();
    request["ContactChannelId"] = json!(channel_arn);
    emulator.call("UpdateContactChannel", request).await
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

/// The entry `ListContactChannels` reports for `channel_arn` under `contact_arn`.
async fn listed_channel(
    emulator: &Emulator,
    contact_arn: &str,
    channel_arn: &str,
    case: &str,
) -> Value {
    let (status, body) = emulator
        .call("ListContactChannels", json!({"ContactId": contact_arn}))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListContactChannels to answer 200, got {status} {body:?}"
    );
    body["ContactChannels"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|entry| entry["ContactChannelArn"].as_str() == Some(channel_arn))
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected {channel_arn} among the listed channels, got {body:?}")
        })
}

/// Everything an update could write about one stored channel.
#[derive(Debug, PartialEq)]
struct ChannelSnapshot {
    arn: String,
    name: String,
    address: Option<String>,
    status: String,
    /// The activation code outstanding for the channel, which an address change supersedes.
    code: Option<String>,
}

/// Every stored channel, read straight from the store: what a refused update must leave
/// exactly as it found it.
fn stored_channels(emulator: &Emulator) -> Vec<ChannelSnapshot> {
    let state = service_state(&*emulator.engine).expect("the engine holds this service's state");
    let state = state.lock();
    state
        .contact_channels
        .values()
        .map(|channel| ChannelSnapshot {
            arn: channel.arn.clone(),
            name: channel.name.clone(),
            address: channel.delivery_address.simple_address.clone(),
            status: channel.activation_status.clone(),
            code: channel.activation_code.clone(),
        })
        .collect()
}

/// A well-formed `SsmContactsArn` naming a contact channel nothing ever created.
fn absent_channel_arn(alias: &str) -> String {
    format!("arn:aws:ssm-contacts:{REGION}:{ACCOUNT}:contact-channel/{alias}/never-created")
}

// ---------------------------------------------------------------------------
// Rule: named members replace, omitted members are untouched
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_update_replaces_the_members_it_names_and_leaves_the_rest() {
    for base in BASE_CHANNELS {
        for row in UPDATE_ROWS {
            let case = format!("{} updated by {}", base.case, row.case);
            let emulator = Emulator::new();
            let (contact, channel, status_before) =
                base_channel(&emulator, "oncall-primary", base).await;

            let patch = (row.patch)(base.name, base.address);
            let (status, body) = update(&emulator, &channel, &patch).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected UpdateContactChannel to answer 200, got {status} \
                 {body:?}"
            );
            assert!(
                body == json!({}) || body.is_null(),
                "case {case:?}: UpdateContactChannel models an empty output, got {body:?}"
            );

            let (expected_name, expected_address, expected_status) =
                expected_after(base.name, base.address, &patch, &status_before);

            let stored = get_channel(&emulator, &channel, &case).await;
            assert_eq!(
                stored["Name"].as_str(),
                Some(expected_name.as_str()),
                "case {case:?}: expected Name {expected_name:?}, got {:?}",
                stored["Name"]
            );
            assert_eq!(
                stored["DeliveryAddress"]["SimpleAddress"].as_str(),
                Some(expected_address.as_str()),
                "case {case:?}: expected SimpleAddress {expected_address:?}, got {:?}",
                stored["DeliveryAddress"]
            );
            assert_eq!(
                stored["ActivationStatus"].as_str(),
                Some(expected_status.as_str()),
                "case {case:?}: expected ActivationStatus {expected_status:?}, got {:?}",
                stored["ActivationStatus"]
            );
            assert!(
                ACTIVATION_STATUS_VALUES.contains(&expected_status.as_str()),
                "case {case:?}: the expected status must be a modelled one, \
                 {ACTIVATION_STATUS_VALUES:?} does not hold {expected_status:?}"
            );
            // The identity the request addressed, and the members the input cannot carry,
            // never move.
            assert_eq!(
                stored["ContactChannelArn"].as_str(),
                Some(channel.as_str()),
                "case {case:?}: an update must not change the channel's ARN, got {stored:?}"
            );
            assert_eq!(
                stored["ContactArn"].as_str(),
                Some(contact.as_str()),
                "case {case:?}: an update must not move the channel to another contact, got \
                 {stored:?}"
            );
            assert_eq!(
                stored["Type"].as_str(),
                Some(base.channel_type),
                "case {case:?}: an update must not change the Type, got {stored:?}"
            );

            // The listing reports the same channel as the read does.
            let listed = listed_channel(&emulator, &contact, &channel, &case).await;
            assert_eq!(
                listed["Name"].as_str(),
                Some(expected_name.as_str()),
                "case {case:?}: expected the listing to report Name {expected_name:?}, got \
                 {listed:?}"
            );
            assert_eq!(
                listed["DeliveryAddress"]["SimpleAddress"].as_str(),
                Some(expected_address.as_str()),
                "case {case:?}: expected the listing to report SimpleAddress \
                 {expected_address:?}, got {listed:?}"
            );
            assert_eq!(
                listed["ActivationStatus"].as_str(),
                Some(expected_status.as_str()),
                "case {case:?}: expected the listing to report ActivationStatus \
                 {expected_status:?}, got {listed:?}"
            );
        }
    }
}

#[tokio::test]
async fn updates_accumulate_and_applying_one_twice_changes_nothing_more() {
    let base = &BASE_CHANNELS[0];
    let case = "every update row applied in sequence to one channel";
    let emulator = Emulator::new();
    let (contact, channel, _) = base_channel(&emulator, "oncall-primary", base).await;

    // The channel as created; each patch overlays the state the previous one left, so the
    // expectation is carried forward rather than recomputed from the create.
    let mut name = base.name.to_string();
    let mut address = base.address.to_string();
    let mut status = ACTIVATED.to_string();

    for row in UPDATE_ROWS {
        let case = format!("{case}, at {}", row.case);
        // The row is built against the channel as it stands NOW, so the rows naming "the
        // value it already carries" really do name the current value.
        let patch = (row.patch)(&name, &address);
        let (expected_name, expected_address, expected_status) =
            expected_after(&name, &address, &patch, &status);

        // Applying the same update twice must leave the same channel: the write is
        // determined by the request, not by how many times it was sent.
        for attempt in 1..=2 {
            let (http, body) = update(&emulator, &channel, &patch).await;
            assert_eq!(
                http, 200,
                "case {case:?} attempt {attempt}: expected 200, got {http} {body:?}"
            );
            let stored = get_channel(&emulator, &channel, &case).await;
            assert_eq!(
                stored["Name"].as_str(),
                Some(expected_name.as_str()),
                "case {case:?} attempt {attempt}: expected Name {expected_name:?}, got {:?}",
                stored["Name"]
            );
            assert_eq!(
                stored["DeliveryAddress"]["SimpleAddress"].as_str(),
                Some(expected_address.as_str()),
                "case {case:?} attempt {attempt}: expected SimpleAddress \
                 {expected_address:?}, got {:?}",
                stored["DeliveryAddress"]
            );
            assert_eq!(
                stored["ActivationStatus"].as_str(),
                Some(expected_status.as_str()),
                "case {case:?} attempt {attempt}: expected ActivationStatus \
                 {expected_status:?}, got {:?}",
                stored["ActivationStatus"]
            );
        }

        name = expected_name;
        address = expected_address;
        status = expected_status;
    }

    // One channel throughout: an update never mints a second one.
    let listed = listed_channel(&emulator, &contact, &channel, case).await;
    assert_eq!(
        listed["Name"].as_str(),
        Some(name.as_str()),
        "case {case:?}: expected the listing to end at {name:?}, got {listed:?}"
    );
    assert_eq!(
        stored_channels(&emulator).len(),
        1,
        "case {case:?}: expected exactly one stored channel, got {:?}",
        stored_channels(&emulator)
    );
}

// ---------------------------------------------------------------------------
// Rule: an ACTIVATED status attests the address the code went to
// ---------------------------------------------------------------------------

/// Moving the delivery address returns the channel to `NOT_ACTIVATED`, supersedes the code
/// that was delivered to the OLD address, and the code delivered to the new one activates it
/// again. A rename leaves the status and the outstanding code untouched.
#[tokio::test]
async fn changing_the_delivery_address_requires_activating_the_channel_again() {
    for base in BASE_CHANNELS {
        let case = format!("{}, its address moved", base.case);
        let emulator = Emulator::new();
        let (_, channel, _) = base_channel(&emulator, "oncall-primary", base).await;
        activate(&emulator, &channel, &case).await;
        let code_before = delivered_code(&emulator, &channel)
            .unwrap_or_else(|| panic!("case {case:?}: the activated channel holds its code"));
        assert_eq!(
            get_channel(&emulator, &channel, &case).await["ActivationStatus"].as_str(),
            Some(ACTIVATED),
            "case {case:?}: the fixture must start ACTIVATED"
        );

        // A rename is not an address change: it touches neither the status nor the code.
        let (status, body) = update(&emulator, &channel, &json!({"Name": "renamed"})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected a rename to answer 200, got {status} {body:?}"
        );
        assert_eq!(
            get_channel(&emulator, &channel, &case).await["ActivationStatus"].as_str(),
            Some(ACTIVATED),
            "case {case:?}: a rename must leave the channel ACTIVATED"
        );
        assert_eq!(
            delivered_code(&emulator, &channel).as_deref(),
            Some(code_before.as_str()),
            "case {case:?}: a rename must not supersede the outstanding activation code"
        );

        // Naming the address it already carries changes nothing either.
        let (status, body) = update(
            &emulator,
            &channel,
            &json!({"DeliveryAddress": {"SimpleAddress": base.address}}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected re-naming the same address to answer 200, got {status} \
             {body:?}"
        );
        assert_eq!(
            get_channel(&emulator, &channel, &case).await["ActivationStatus"].as_str(),
            Some(ACTIVATED),
            "case {case:?}: writing the stored address must leave the channel ACTIVATED"
        );

        // Moving it does.
        let moved = "moved@example.com";
        let (status, body) = update(
            &emulator,
            &channel,
            &json!({"DeliveryAddress": {"SimpleAddress": moved}}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the address change to answer 200, got {status} {body:?}"
        );
        let stored = get_channel(&emulator, &channel, &case).await;
        assert_eq!(
            stored["ActivationStatus"].as_str(),
            Some(NOT_ACTIVATED),
            "case {case:?}: a channel pointed at an unverified address must read back \
             NOT_ACTIVATED, got {stored:?}"
        );
        assert_eq!(
            stored["DeliveryAddress"]["SimpleAddress"].as_str(),
            Some(moved),
            "case {case:?}: expected the new address to be stored, got {stored:?}"
        );

        // The code delivered to the address the channel LEFT no longer activates it.
        let (status, body) = emulator
            .call(
                "ActivateContactChannel",
                json!({"ContactChannelId": channel, "ActivationCode": code_before}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the superseded code to be refused, got {status} {body:?}"
        );
        assert!(
            error_code(&body).contains("ValidationException"),
            "case {case:?}: expected a ValidationException, got {body:?}"
        );

        // A fresh code went to the new address, and it activates the channel.
        let code_after = delivered_code(&emulator, &channel)
            .unwrap_or_else(|| panic!("case {case:?}: a code was delivered to the new address"));
        assert_ne!(
            code_after, code_before,
            "case {case:?}: the code delivered to the new address must supersede the old one"
        );
        let (status, body) = emulator
            .call(
                "ActivateContactChannel",
                json!({"ContactChannelId": channel, "ActivationCode": code_after}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the new code to activate the channel, got {status} \
             {body:?}"
        );
        assert_eq!(
            get_channel(&emulator, &channel, &case).await["ActivationStatus"].as_str(),
            Some(ACTIVATED),
            "case {case:?}: the re-activated channel must read back ACTIVATED"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule: a channel's Name stays unique within its contact
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_rename_onto_a_siblings_name_is_a_conflict() {
    // The names span the modelled `ChannelName` shape, exactly as the create-side uniqueness
    // suite spans it: a plain name, a separated/dotted/hyphenated one, non-ASCII letters, and
    // the published bounds 1 and 255.
    let sibling_names: &[(&str, String)] = &[
        ("a plain name", "pager".to_string()),
        (
            "a separated, dotted and hyphenated name",
            "mobile sms.work-1".to_string(),
        ),
        ("non-ASCII letters", "Überwachung téléphone".to_string()),
        ("the published minimum length of 1", "e".to_string()),
        ("the published maximum length of 255", "n".repeat(255)),
    ];

    for (name_case, sibling_name) in sibling_names {
        let case = format!("a rename onto a sibling carrying {name_case}");
        let emulator = Emulator::new();
        let base = &BASE_CHANNELS[0];
        let (contact, channel, _) = base_channel(&emulator, "oncall-primary", base).await;
        let sibling = create_channel(
            &emulator,
            json!({
                "ContactId": contact,
                "Name": sibling_name,
                "Type": "SMS",
                "DeliveryAddress": {"SimpleAddress": "+15550111"},
            }),
            &case,
        )
        .await;
        let before = stored_channels(&emulator);

        let (status, body) = update(&emulator, &channel, &json!({"Name": sibling_name})).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected a rename onto a sibling's name to answer 409, got \
             {status} {body:?}"
        );
        assert!(
            error_code(&body).contains("ConflictException"),
            "case {case:?}: expected a ConflictException, got {body:?}"
        );
        assert_eq!(
            body["ResourceId"].as_str(),
            Some(sibling.as_str()),
            "case {case:?}: expected the conflict to name the channel in use, got {body:?}"
        );
        assert_eq!(
            body["ResourceType"].as_str(),
            Some("contact-channel"),
            "case {case:?}: expected the conflict to name the resource type, got {body:?}"
        );
        assert_eq!(
            stored_channels(&emulator),
            before,
            "case {case:?}: a refused rename must write nothing"
        );

        // A rename onto the channel's OWN name names only itself, so it is not a conflict.
        let (status, body) = update(&emulator, &channel, &json!({"Name": base.name})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected a channel renamed to its own name to answer 200, got \
             {status} {body:?}"
        );
        assert_eq!(
            get_channel(&emulator, &channel, &case).await["Name"].as_str(),
            Some(base.name),
            "case {case:?}: the no-op rename must leave the name as it was"
        );
    }
}

#[tokio::test]
async fn a_rename_may_take_a_name_another_contacts_channel_carries() {
    let emulator = Emulator::new();
    let case = "a rename onto a name a channel of ANOTHER contact carries";
    let base = &BASE_CHANNELS[0];
    let (_, channel, _) = base_channel(&emulator, "oncall-primary", base).await;
    let other_contact = onboarded_contact(&emulator, "oncall-secondary").await;
    let other_channel = create_channel(
        &emulator,
        json!({
            "ContactId": other_contact,
            "Name": "shared-name",
            "Type": "EMAIL",
            "DeliveryAddress": {"SimpleAddress": "second@example.com"},
        }),
        case,
    )
    .await;

    let (status, body) = update(&emulator, &channel, &json!({"Name": "shared-name"})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the name is unique only WITHIN a contact, got {status} {body:?}"
    );
    assert_eq!(
        get_channel(&emulator, &channel, case).await["Name"].as_str(),
        Some("shared-name"),
        "case {case:?}: expected the rename to be stored"
    );
    assert_eq!(
        get_channel(&emulator, &other_channel, case).await["Name"].as_str(),
        Some("shared-name"),
        "case {case:?}: the other contact's channel must be untouched"
    );
}

// ---------------------------------------------------------------------------
// Rule: the refusals, none of which write
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_update_of_an_unstored_channel_answers_404() {
    let emulator = Emulator::new();
    // One live channel, so "absent" cannot be confused with "the store is empty", plus a
    // contact ARN and a deleted channel's ARN: every well-formed ARN that names no STORED
    // CHANNEL answers the same way.
    let base = &BASE_CHANNELS[0];
    let (contact, _live, _) = base_channel(&emulator, "oncall-primary", base).await;
    let doomed = create_channel(
        &emulator,
        json!({
            "ContactId": contact,
            "Name": "doomed",
            "Type": "EMAIL",
            "DeliveryAddress": {"SimpleAddress": "doomed@example.com"},
        }),
        "a channel to delete",
    )
    .await;
    let (status, body) = emulator
        .call(
            "DeleteContactChannel",
            json!({"ContactChannelId": doomed.clone()}),
        )
        .await;
    assert_eq!(
        status, 200,
        "expected DeleteContactChannel to answer 200, got {status} {body:?}"
    );

    let absent: &[(&str, String)] = &[
        (
            "an ARN naming a channel nothing created",
            absent_channel_arn("oncall-primary"),
        ),
        ("a contact's ARN", contact.clone()),
        ("a deleted channel's ARN", doomed),
    ];

    for (target_case, target) in absent {
        for row in UPDATE_ROWS {
            let case = format!("{target_case}, updated by {}", row.case);
            let before = stored_channels(&emulator);
            let (status, body) =
                update(&emulator, target, &(row.patch)(base.name, base.address)).await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected HTTP 404, got {status} {body:?}"
            );
            assert!(
                error_code(&body).contains("ResourceNotFoundException"),
                "case {case:?}: expected a ResourceNotFoundException, got {body:?}"
            );
            assert_eq!(
                stored_channels(&emulator),
                before,
                "case {case:?}: a refused update must write nothing"
            );
        }
    }
}

#[tokio::test]
async fn an_update_naming_no_updatable_member_is_refused() {
    let emulator = Emulator::new();
    let base = &BASE_CHANNELS[0];
    let (_, channel, _) = base_channel(&emulator, "oncall-primary", base).await;

    // Both the request that names neither member and the one whose `DeliveryAddress` carries
    // no `SimpleAddress` describe no address to engage.
    let refused: &[(&str, Value)] = &[
        ("neither Name nor DeliveryAddress", json!({})),
        (
            "a DeliveryAddress carrying no SimpleAddress",
            json!({"DeliveryAddress": {}}),
        ),
        (
            "a DeliveryAddress whose SimpleAddress is null",
            json!({"DeliveryAddress": {"SimpleAddress": null}}),
        ),
    ];

    for (case, patch) in refused {
        let before = stored_channels(&emulator);
        let (status, body) = update(&emulator, &channel, patch).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body:?}"
        );
        assert!(
            error_code(&body).contains("ValidationException"),
            "case {case:?}: expected a ValidationException, got {body:?}"
        );
        assert_eq!(
            stored_channels(&emulator),
            before,
            "case {case:?}: a refused update must write nothing"
        );
    }
}

/// A member outside its modelled constraint is refused with HTTP 400 before the handler
/// runs — asserted on each constraint `UpdateContactChannel`'s input declares, at one past
/// each published bound.
#[tokio::test]
async fn members_outside_their_modelled_constraint_are_refused() {
    let emulator = Emulator::new();
    let base = &BASE_CHANNELS[0];
    let (_, channel, _) = base_channel(&emulator, "oncall-primary", base).await;
    let before = stored_channels(&emulator);

    let refused: &[(&str, Value)] = &[
        // `ContactChannelId` is required (SsmContactsArn, len 1..2048, pattern ^arn:...$).
        ("ContactChannelId absent", json!({"Name": "x"})),
        (
            "ContactChannelId outside its modelled pattern",
            json!({"ContactChannelId": "not-an-arn", "Name": "x"}),
        ),
        (
            "ContactChannelId shorter than its modelled minimum length",
            json!({"ContactChannelId": "", "Name": "x"}),
        ),
        // `Name` is ChannelName: len 1..255, pattern ^[\p{L}\p{Z}\p{N}_.\-]*$.
        (
            "a Name one past the published maximum length of 255",
            json!({"ContactChannelId": channel, "Name": "n".repeat(256)}),
        ),
        (
            "a Name shorter than the published minimum length of 1",
            json!({"ContactChannelId": channel, "Name": ""}),
        ),
        (
            "a Name outside the published pattern",
            json!({"ContactChannelId": channel, "Name": "no/slashes!"}),
        ),
        // `SimpleAddress` is len 1..320.
        (
            "a SimpleAddress one past the published maximum length of 320",
            json!({
                "ContactChannelId": channel,
                "DeliveryAddress": {"SimpleAddress": "a".repeat(321)},
            }),
        ),
        (
            "a SimpleAddress shorter than the published minimum length of 1",
            json!({
                "ContactChannelId": channel,
                "DeliveryAddress": {"SimpleAddress": ""},
            }),
        ),
    ];

    for (case, request) in refused {
        let (status, body) = emulator.call("UpdateContactChannel", request.clone()).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body:?}"
        );
        assert!(
            error_code(&body).contains("ValidationException"),
            "case {case:?}: expected a ValidationException, got {body:?}"
        );
    }

    assert_eq!(
        stored_channels(&emulator),
        before,
        "a refused request must write nothing"
    );
}

// ---------------------------------------------------------------------------
// Rule: the account gate, evaluated before the channel is resolved
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_update_is_refused_when_the_account_has_no_active_replication_set() {
    // The two ways an account has no ACTIVE replication set: none at all, and one that has
    // not finished replicating.
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
        let base = &BASE_CHANNELS[0];
        // The channel is created while the account IS onboarded, so the only thing the
        // update differs in is the precondition.
        let (_, live, _) = base_channel(&emulator, "oncall-primary", base).await;
        let absent = absent_channel_arn("never-created");
        let before = stored_channels(&emulator);

        for (target_case, target) in [("a stored channel", &live), ("an absent channel", &absent)] {
            for row in UPDATE_ROWS {
                set_replication_set(&emulator, replication_set.clone());
                let case = format!("{account_case}, {target_case}, updated by {}", row.case);

                let (status, body) =
                    update(&emulator, target, &(row.patch)(base.name, base.address)).await;
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected the account precondition to answer 400, got \
                     {status} {body:?}"
                );
                assert!(
                    error_code(&body).contains("ValidationException"),
                    "case {case:?}: expected a ValidationException — NOT a \
                     ResourceNotFoundException, even for an absent channel — got {body:?}"
                );

                // Nothing was written: with the account onboarded again, the store still
                // holds exactly what it did.
                onboard(&emulator);
                assert_eq!(
                    stored_channels(&emulator),
                    before,
                    "case {case:?}: a refused update must write nothing"
                );
            }
        }
    }
}
