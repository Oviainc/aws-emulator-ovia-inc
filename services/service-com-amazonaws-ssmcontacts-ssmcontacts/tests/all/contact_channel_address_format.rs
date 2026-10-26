// SPDX-License-Identifier: CC0-1.0

//! A contact channel's delivery address, end to end over the wire.
//!
//! The rule under test:
//!
//! > A contact channel's `DeliveryAddress.SimpleAddress` is what Incident Manager engages the
//! > contact THROUGH, and the form it has to take follows the channel's `ChannelType`: a
//! > telephone channel (`SMS`, `VOICE`) carries '+' and the country code and phone number, an
//! > `EMAIL` channel carries a standard email format
//! > (`smithy_query(query="doc",shape="ContactChannelAddress",member="SimpleAddress")`). An
//! > address in another form reaches nothing through a channel of that type, so every
//! > operation that writes a channel's address — `CreateContactChannel` and
//! > `UpdateContactChannel` — refuses it with the modelled `ValidationException` (HTTP 400)
//! > naming `deliveryAddress.simpleAddress`, mints nothing, delivers no activation code and
//! > leaves a stored channel exactly as it was.
//!
//! The suites are a CROSS PRODUCT rather than a list of cases: one catalog of well-formed
//! addresses, each labelled with the form it is in, applied to the model's whole
//! `ChannelType` value set — so every row is accepted by the types that prescribe its form
//! and refused by the ones that do not. A second catalog carries addresses in NO form the
//! model documents, which every channel type refuses. The three cases the issue cited
//! (`EMAIL` with a phone number, `SMS` with an email address, `VOICE` with neither) are rows
//! among them.

use serde_json::{json, Value};

use service_com_amazonaws_ssmcontacts_ssmcontacts::ops::types::CHANNEL_TYPE_VALUES;
use service_com_amazonaws_ssmcontacts_ssmcontacts::state::service_state;

use crate::contact_lifecycle::{error_code, Emulator};
use crate::data_plane_precondition::{create_channel, delivered_code, onboarded_contact};

/// The member every refusal of a delivery address names.
const MEMBER: &str = "deliveryAddress.simpleAddress";

/// The forms the model documents for a delivery address, one per `ChannelType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Form {
    /// "EMAIL - any standard email format".
    Email,
    /// "SMS - '+' followed by the country code and phone number" and the same for "VOICE".
    Phone,
}

/// The form `channel_type` prescribes for its delivery address.
///
/// The mapping is the model's: the per-`ChannelType` formats documented on
/// `ContactChannelAddress$SimpleAddress`, over the whole `ChannelType` value set
/// (`CHANNEL_TYPE_VALUES`, rendered from the model into the crate).
fn form_of(channel_type: &str) -> Form {
    match channel_type {
        "EMAIL" => Form::Email,
        "SMS" | "VOICE" => Form::Phone,
        other => panic!("the model declares no ChannelType {other:?}"),
    }
}

/// One address, and the form it is in.
struct AddressRow {
    case: &'static str,
    address: String,
    form: Form,
}

/// Addresses that ARE in one of the documented forms, so a channel of a type prescribing that
/// form accepts them and a channel of any other type refuses them.
///
/// The rows differ along what the rule generalises over: the local part's and the domain's
/// shape and label count for an email address, the country code's width and the digit count
/// for a telephone number, and the published bound each form reaches — `SimpleAddress`' own
/// published maximum length of 320
/// (`smithy_query(query="shape",shape="ContactChannelAddress")`), which an email address
/// reaches with a 64-character local part and a 255-character domain, and the 15 digits ITU-T
/// E.164 caps an international telephone number at. The issue's own two well-formed addresses
/// are the first row of each form.
fn well_formed_addresses() -> Vec<AddressRow> {
    vec![
        AddressRow {
            case: "the issue's own email address",
            address: "ops@example.com".to_string(),
            form: Form::Email,
        },
        AddressRow {
            case: "an email address with a dotted local part, a '+' tag and four domain labels",
            address: "first.last+oncall@paging.eu-west-1.example.com".to_string(),
            form: Form::Email,
        },
        AddressRow {
            case: "an email address at SimpleAddress' published maximum length of 320",
            address: longest_email_address(),
            form: Form::Email,
        },
        AddressRow {
            case: "a short email address: one-character local part, two domain labels",
            address: "a@b.co".to_string(),
            form: Form::Email,
        },
        AddressRow {
            case: "the issue's own phone number",
            address: "+15550100".to_string(),
            form: Form::Phone,
        },
        AddressRow {
            case: "a phone number with a two-digit country code",
            address: "+442071838750".to_string(),
            form: Form::Phone,
        },
        AddressRow {
            case: "a phone number at E.164's maximum of 15 digits",
            address: format!("+{}", "9".repeat(15)),
            form: Form::Phone,
        },
        AddressRow {
            case: "the shortest phone number the form admits: a country code and one digit",
            address: "+12".to_string(),
            form: Form::Phone,
        },
    ]
}

/// An email address exactly at `SimpleAddress`' published maximum length of 320: a
/// 64-character local part, '@', and a 255-character domain of four 63-character labels.
fn longest_email_address() -> String {
    let domain = vec!["b".repeat(63); 4].join(".");
    let address = format!("{}@{domain}", "a".repeat(64));
    assert_eq!(
        address.len(),
        320,
        "the longest email address must sit on SimpleAddress' published maximum, got {}",
        address.len()
    );
    address
}

/// Addresses in NEITHER documented form, which every channel type refuses: the forms are what
/// the rule admits, so an address outside both is reachable through no channel at all.
///
/// The rows differ along how they miss: no separator, a separator with nothing on one side,
/// whitespace inside, a telephone number without its '+', one with separators between its
/// digits, one whose country code begins with the digit E.164 assigns none from, one digit
/// past E.164's maximum of 15, and the single character `SimpleAddress`' published minimum
/// length of 1 admits — which no documented form reaches.
fn malformed_addresses() -> Vec<(&'static str, String)> {
    vec![
        (
            "the issue's third case: neither an email address nor a phone number",
            "not-a-number".to_string(),
        ),
        ("an email address missing its domain", "ops@".to_string()),
        (
            "an email address missing its local part",
            "@example.com".to_string(),
        ),
        (
            "an address carrying whitespace",
            "ops team@example.com".to_string(),
        ),
        (
            "a phone number without its leading '+'",
            "15550100".to_string(),
        ),
        (
            "a phone number with separators between its digits",
            "+1 555-0100".to_string(),
        ),
        (
            "a phone number whose country code begins with 0, which E.164 assigns none from",
            "+0155501000".to_string(),
        ),
        (
            "a phone number one digit past E.164's maximum of 15",
            format!("+{}", "9".repeat(16)),
        ),
        (
            "the single character SimpleAddress' published minimum length of 1 admits",
            "x".to_string(),
        ),
    ]
}

/// The `CreateContactChannel` body for one channel.
fn create_request(contact_arn: &str, name: &str, channel_type: &str, address: &str) -> Value {
    json!({
        "ContactId": contact_arn,
        "Name": name,
        "Type": channel_type,
        "DeliveryAddress": {"SimpleAddress": address},
    })
}

/// Every stored channel's `(ARN, delivery address, activation code)`, read straight from the
/// store: a refused call must leave this exactly as it found it, and must never put a code in
/// a channel's mailbox.
fn stored_addresses(emulator: &Emulator) -> Vec<(String, Option<String>, Option<String>)> {
    let state = service_state(&*emulator.engine).expect("the engine holds this service's state");
    let state = state.lock();
    state
        .contact_channels
        .values()
        .map(|channel| {
            (
                channel.arn.clone(),
                channel.delivery_address.simple_address.clone(),
                channel.activation_code.clone(),
            )
        })
        .collect()
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

/// Assert `body` is the member-level `ValidationException` the rule refuses an address with.
fn assert_address_refusal(status: u16, body: &Value, case: &str) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected an address outside its type's form to answer 400, got \
         {status} {body:?}"
    );
    assert!(
        error_code(body).contains("ValidationException"),
        "case {case:?}: expected a ValidationException, got {body:?}"
    );
    let message = body["Message"].as_str().unwrap_or_default();
    assert!(
        message.contains(MEMBER),
        "case {case:?}: expected the refusal to name {MEMBER:?}, got {message:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule: a create accepts an address exactly when its type prescribes that form
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_create_accepts_an_address_exactly_when_its_type_prescribes_that_form() {
    for row in well_formed_addresses() {
        for channel_type in CHANNEL_TYPE_VALUES {
            let case = format!("{} as the address of a {channel_type} channel", row.case);
            let emulator = Emulator::new();
            let contact = onboarded_contact(&emulator, "oncall-primary").await;
            let request = create_request(&contact, "paging", channel_type, &row.address);

            if form_of(channel_type) == row.form {
                let channel = create_channel(&emulator, request, &case).await;

                // Round trip: the stored channel reports the type and the address created.
                let stored = get_channel(&emulator, &channel, &case).await;
                assert_eq!(
                    stored["Type"].as_str(),
                    Some(*channel_type),
                    "case {case:?}: expected the created Type, got {stored:?}"
                );
                assert_eq!(
                    stored["DeliveryAddress"]["SimpleAddress"].as_str(),
                    Some(row.address.as_str()),
                    "case {case:?}: expected the created address, got {:?}",
                    stored["DeliveryAddress"]
                );
                // An accepted address IS a channel Incident Manager can engage, so the
                // activation code went to it.
                assert!(
                    delivered_code(&emulator, &channel).is_some(),
                    "case {case:?}: expected an activation code delivered to the channel"
                );
                continue;
            }

            let (status, body) = emulator.call("CreateContactChannel", request).await;
            assert_address_refusal(status, &body, &case);
            assert!(
                body["ContactChannelArn"].is_null(),
                "case {case:?}: a refused create must mint no ARN, got {body:?}"
            );
            // Nothing minted, so nothing to read back and no mailbox to deliver a code to.
            assert!(
                stored_addresses(&emulator).is_empty(),
                "case {case:?}: a refused create must store no channel, got {:?}",
                stored_addresses(&emulator)
            );
            let (status, body) = emulator
                .call("ListContactChannels", json!({"ContactId": contact}))
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected ListContactChannels to answer 200, got {status} \
                 {body:?}"
            );
            assert_eq!(
                body["ContactChannels"].as_array().map(Vec::len),
                Some(0),
                "case {case:?}: expected the contact to hold no channel, got {body:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_create_refuses_an_address_in_no_documented_form_whatever_the_type() {
    for (row_case, address) in malformed_addresses() {
        for channel_type in CHANNEL_TYPE_VALUES {
            let case = format!("{row_case} as the address of a {channel_type} channel");
            let emulator = Emulator::new();
            let contact = onboarded_contact(&emulator, "oncall-primary").await;

            let (status, body) = emulator
                .call(
                    "CreateContactChannel",
                    create_request(&contact, "paging", channel_type, &address),
                )
                .await;
            assert_address_refusal(status, &body, &case);
            assert!(
                stored_addresses(&emulator).is_empty(),
                "case {case:?}: a refused create must store no channel, got {:?}",
                stored_addresses(&emulator)
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule: an update may only move an address within the form its type prescribes
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_update_may_not_point_a_channel_at_an_address_its_type_does_not_engage() {
    for channel_type in CHANNEL_TYPE_VALUES {
        // The channel starts at the first address of its own form, and is ACTIVATED, so a
        // refused update is visible in three places at once: the stored address, the
        // activation status the address attests, and the outstanding code.
        let form = form_of(channel_type);
        let rows = well_formed_addresses();
        let own_form = rows
            .iter()
            .find(|row| row.form == form)
            .expect("the catalog carries an address of every documented form");
        let emulator = Emulator::new();
        let contact = onboarded_contact(&emulator, "oncall-primary").await;
        let channel = create_channel(
            &emulator,
            create_request(&contact, "paging", channel_type, &own_form.address),
            channel_type,
        )
        .await;
        let code = delivered_code(&emulator, &channel)
            .unwrap_or_else(|| panic!("a code was delivered to the {channel_type} channel"));
        let (status, body) = emulator
            .call(
                "ActivateContactChannel",
                json!({"ContactChannelId": channel, "ActivationCode": code}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {channel_type:?}: expected the channel to activate, got {status} {body:?}"
        );
        let before = stored_addresses(&emulator);

        // Every address the channel's own type does not prescribe, well-formed in another
        // form or in none, is refused and writes nothing.
        let refused: Vec<(&str, String)> = rows
            .iter()
            .filter(|row| row.form != form)
            .map(|row| (row.case, row.address.clone()))
            .chain(malformed_addresses())
            .collect();
        for (row_case, address) in refused {
            let case = format!("a {channel_type} channel pointed at {row_case}");
            let (status, body) = emulator
                .call(
                    "UpdateContactChannel",
                    json!({
                        "ContactChannelId": channel,
                        "DeliveryAddress": {"SimpleAddress": address},
                    }),
                )
                .await;
            assert_address_refusal(status, &body, &case);
            assert_eq!(
                stored_addresses(&emulator),
                before,
                "case {case:?}: a refused update must write nothing"
            );
            let stored = get_channel(&emulator, &channel, &case).await;
            assert_eq!(
                stored["ActivationStatus"].as_str(),
                Some("ACTIVATED"),
                "case {case:?}: a refused update must leave the channel activated, got \
                 {stored:?}"
            );
        }

        // An address in the form its type DOES prescribe is accepted, and read back.
        for row in rows.iter().filter(|row| row.form == form) {
            let case = format!("a {channel_type} channel moved to {}", row.case);
            let (status, body) = emulator
                .call(
                    "UpdateContactChannel",
                    json!({
                        "ContactChannelId": channel,
                        "DeliveryAddress": {"SimpleAddress": row.address},
                    }),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected an address of the channel's own form to answer 200, \
                 got {status} {body:?}"
            );
            let stored = get_channel(&emulator, &channel, &case).await;
            assert_eq!(
                stored["DeliveryAddress"]["SimpleAddress"].as_str(),
                Some(row.address.as_str()),
                "case {case:?}: expected the new address to be stored, got {:?}",
                stored["DeliveryAddress"]
            );
        }
    }
}
