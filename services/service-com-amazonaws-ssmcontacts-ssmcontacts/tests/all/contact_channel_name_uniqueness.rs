// SPDX-License-Identifier: CC0-1.0

//! A contact channel's `Name` identifies it within its contact: `CreateContactChannel`
//! refuses a name a live channel of the same contact already carries.
//!
//! The suite is a TABLE over what the rule generalises over — the `Name`'s form and length,
//! the `Type` and `DeliveryAddress` the repeat carries, and whether it defers activation —
//! plus the rule's SCOPE (another contact may hold the same channel name), its LIFETIME (a
//! name freed by a delete may be used again), and its precedence against the idempotency
//! replay. The issue's own case (`"email"`, `EMAIL`, repeated identically) is one row.

use serde_json::{json, Value};

use crate::contact_lifecycle::{error_code, Emulator};

/// One row of the duplicate-name table.
struct NameRow {
    case: &'static str,
    /// The channel name both creates carry.
    name: String,
    channel_type: &'static str,
    address: &'static str,
    /// What the SECOND create names instead — the name alone decides the verdict, so a
    /// repeat that changes the type, the address or the activation deferral is still a
    /// conflict.
    repeat_type: &'static str,
    repeat_address: &'static str,
    repeat_defer: Option<bool>,
}

/// The names span the modelled `ChannelName` shape: `len 1..255`, pattern
/// `^[\p{L}\p{Z}\p{N}_.\-]*$` (`smithy_query(query="shape",shape="ChannelName")`), so the
/// rows carry letters, a separator, digits, `_`, `.`, `-`, non-ASCII letters, the published
/// minimum length of 1 and the published maximum of 255. `Type` spans the whole
/// `ChannelType` value set with the address format each one documents.
fn duplicate_name_table() -> Vec<NameRow> {
    vec![
        NameRow {
            case: "the issue's own case: an EMAIL channel named \"email\", repeated identically",
            name: "email".to_string(),
            channel_type: "EMAIL",
            address: "ops@example.com",
            repeat_type: "EMAIL",
            repeat_address: "ops@example.com",
            repeat_defer: None,
        },
        NameRow {
            case: "a separated, dotted and hyphenated name repeated as a different type",
            name: "mobile sms.work-1".to_string(),
            channel_type: "SMS",
            address: "+15550100",
            repeat_type: "VOICE",
            repeat_address: "+15550199",
            repeat_defer: Some(true),
        },
        NameRow {
            case: "a VOICE channel whose name is underscored digits, repeated as EMAIL",
            name: "_4155550101".to_string(),
            channel_type: "VOICE",
            address: "+15550101",
            repeat_type: "EMAIL",
            repeat_address: "voice@example.com",
            repeat_defer: Some(false),
        },
        NameRow {
            case: "a name of non-ASCII letters",
            name: "Überwachung téléphone".to_string(),
            channel_type: "SMS",
            address: "+15550102",
            repeat_type: "SMS",
            repeat_address: "+15550102",
            repeat_defer: Some(true),
        },
        NameRow {
            case: "a name on ChannelName's published minimum length of 1",
            name: "e".to_string(),
            channel_type: "EMAIL",
            address: "e@example.com",
            repeat_type: "EMAIL",
            repeat_address: "e@example.com",
            repeat_defer: None,
        },
        NameRow {
            case: "a name on ChannelName's published maximum length of 255",
            name: "n".repeat(255),
            channel_type: "EMAIL",
            address: "long@example.com",
            repeat_type: "EMAIL",
            repeat_address: "long@example.com",
            repeat_defer: Some(false),
        },
    ]
}

/// The `CreateContactChannel` body for `name` under `contact_arn`.
fn channel_request(
    contact_arn: &str,
    name: &str,
    channel_type: &str,
    address: &str,
    defer: Option<bool>,
) -> Value {
    let mut request = json!({
        "ContactId": contact_arn,
        "Name": name,
        "Type": channel_type,
        "DeliveryAddress": {"SimpleAddress": address},
    });
    if let Some(defer) = defer {
        request["DeferActivation"] = json!(defer);
    }
    request
}

/// Create a contact with `alias`, answering its `ContactArn`.
async fn contact(emulator: &Emulator, alias: &str, case: &str) -> String {
    emulator
        .create_contact(
            json!({"Alias": alias, "Type": "PERSONAL", "Plan": {"Stages": []}}),
            case,
        )
        .await
}

/// Create a channel, asserting the call answered 200, and answer its `ContactChannelArn`.
async fn create_channel(emulator: &Emulator, request: Value, case: &str) -> String {
    let (status, body) = emulator.call("CreateContactChannel", request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CreateContactChannel to answer 200, got {status} {body:?}"
    );
    body["ContactChannelArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a ContactChannelArn, got {body:?}"))
        .to_string()
}

/// Every `(ContactChannelArn, Name)` `ListContactChannels` reports for `contact_arn`.
async fn listed_channels(emulator: &Emulator, contact_arn: &str, case: &str) -> Vec<(String, String)> {
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
        .iter()
        .map(|channel| {
            (
                channel["ContactChannelArn"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                channel["Name"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// A second create under a name one of the contact's channels already carries is refused
/// with `ConflictException` (HTTP 409) naming the channel in use, and changes nothing.
#[tokio::test]
async fn a_repeated_channel_name_under_one_contact_is_a_conflict() {
    for (index, row) in duplicate_name_table().into_iter().enumerate() {
        let emulator = Emulator::new();
        let case = row.case;
        let contact_arn = contact(&emulator, &format!("oncall-{index}"), case).await;

        let arn = create_channel(
            &emulator,
            channel_request(
                &contact_arn,
                &row.name,
                row.channel_type,
                row.address,
                None,
            ),
            case,
        )
        .await;

        let (status, body) = emulator
            .call(
                "CreateContactChannel",
                channel_request(
                    &contact_arn,
                    &row.name,
                    row.repeat_type,
                    row.repeat_address,
                    row.repeat_defer,
                ),
            )
            .await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected a repeated channel name to answer 409, got {status} {body:?}"
        );
        assert!(
            error_code(&body).contains("ConflictException"),
            "case {case:?}: expected a ConflictException, got {body:?}"
        );
        assert_eq!(
            body["ResourceId"].as_str(),
            Some(arn.as_str()),
            "case {case:?}: expected the conflict to name the channel in use, got {body:?}"
        );
        assert_eq!(
            body["ResourceType"].as_str(),
            Some("contact-channel"),
            "case {case:?}: expected the conflict to name the resource type, got {body:?}"
        );

        // The refusal wrote nothing: the contact still holds exactly the first channel,
        // and that channel still carries what the first create stored.
        let listed = listed_channels(&emulator, &contact_arn, case).await;
        assert_eq!(
            listed,
            vec![(arn.clone(), row.name.clone())],
            "case {case:?}: expected exactly the first channel to remain"
        );
        let (status, body) = emulator
            .call("GetContactChannel", json!({"ContactChannelId": arn}))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetContactChannel to answer 200, got {status} {body:?}"
        );
        assert_eq!(
            body["Type"].as_str(),
            Some(row.channel_type),
            "case {case:?}: expected the refused repeat to leave the Type alone, got {body:?}"
        );
        assert_eq!(
            body["DeliveryAddress"]["SimpleAddress"].as_str(),
            Some(row.address),
            "case {case:?}: expected the refused repeat to leave the address alone, got {body:?}"
        );
    }
}

/// The name is unique WITHIN a contact: two contacts may each hold a channel of the same
/// name, and one contact may hold channels of different names.
#[tokio::test]
async fn a_channel_name_is_unique_only_within_its_contact() {
    let emulator = Emulator::new();
    let case = "the same channel name under two contacts";
    let first = contact(&emulator, "oncall-primary", case).await;
    let second = contact(&emulator, "oncall-secondary", case).await;

    let mut minted: Vec<String> = Vec::new();
    for contact_arn in [&first, &second] {
        for name in ["email", "mobile sms"] {
            let arn = create_channel(
                &emulator,
                channel_request(contact_arn, name, "EMAIL", "ops@example.com", None),
                case,
            )
            .await;
            assert!(
                !minted.contains(&arn),
                "case {case:?}: expected a distinct ARN per channel, got {arn:?} twice"
            );
            minted.push(arn);
        }
    }

    for contact_arn in [&first, &second] {
        let mut names: Vec<String> = listed_channels(&emulator, contact_arn, case)
            .await
            .into_iter()
            .map(|(_, name)| name)
            .collect();
        names.sort();
        assert_eq!(
            names,
            vec!["email".to_string(), "mobile sms".to_string()],
            "case {case:?}: expected each contact to hold both channels once"
        );
    }
}

/// The name is held only while the channel lives: once it is deleted the name is free, and
/// the create that takes it mints a NEW channel with its own ARN.
#[tokio::test]
async fn a_channel_name_freed_by_a_delete_can_be_used_again() {
    let emulator = Emulator::new();
    let case = "a channel recreated under the name a deleted one used";
    let contact_arn = contact(&emulator, "oncall-primary", case).await;
    let request = channel_request(&contact_arn, "email", "EMAIL", "ops@example.com", None);

    let first = create_channel(&emulator, request.clone(), case).await;
    let (status, body) = emulator
        .call(
            "DeleteContactChannel",
            json!({"ContactChannelId": first.clone()}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DeleteContactChannel to answer 200, got {status} {body:?}"
    );

    let second = create_channel(&emulator, request.clone(), case).await;
    assert_ne!(
        first, second,
        "case {case:?}: expected the recreated channel to be a new resource with its own ARN"
    );
    let listed = listed_channels(&emulator, &contact_arn, case).await;
    assert_eq!(
        listed,
        vec![(second.clone(), "email".to_string())],
        "case {case:?}: expected the contact to hold exactly the recreated channel"
    );

    // The name is taken again, so the next repeat conflicts with the NEW channel.
    let (status, body) = emulator.call("CreateContactChannel", request).await;
    assert_eq!(
        status, 409,
        "case {case:?}: expected the retaken name to conflict, got {status} {body:?}"
    );
    assert_eq!(
        body["ResourceId"].as_str(),
        Some(second.as_str()),
        "case {case:?}: expected the conflict to name the recreated channel, got {body:?}"
    );
}

/// A retry carrying the `IdempotencyToken` its first call carried is the SAME call, so it
/// replays that call's ARN instead of conflicting with the channel it created; the same
/// name without the token is a different call, and conflicts.
#[tokio::test]
async fn a_retry_with_the_first_calls_token_replays_instead_of_conflicting() {
    let emulator = Emulator::new();
    let case = "a retried CreateContactChannel carrying its own token";
    let contact_arn = contact(&emulator, "oncall-primary", case).await;

    let mut request = channel_request(&contact_arn, "email", "EMAIL", "ops@example.com", None);
    request["IdempotencyToken"] = json!("token-channel-1");

    let first = create_channel(&emulator, request.clone(), case).await;
    let replayed = create_channel(&emulator, request.clone(), case).await;
    assert_eq!(
        first, replayed,
        "case {case:?}: a retried create must answer the ARN its first call minted"
    );

    let mut untokened = request.clone();
    untokened
        .as_object_mut()
        .expect("the request body is a JSON object")
        .remove("IdempotencyToken");
    let (status, body) = emulator.call("CreateContactChannel", untokened).await;
    assert_eq!(
        status, 409,
        "case {case:?}: expected an untokened repeat to answer 409, got {status} {body:?}"
    );
    assert_eq!(
        body["ResourceId"].as_str(),
        Some(first.as_str()),
        "case {case:?}: expected the conflict to name the first channel, got {body:?}"
    );

    let listed = listed_channels(&emulator, &contact_arn, case).await;
    assert_eq!(
        listed.len(),
        1,
        "case {case:?}: expected exactly one channel to exist, got {listed:?}"
    );
}

/// One past `ChannelName`'s published maximum length is a `ValidationException` (HTTP 400)
/// from the constraint boundary, not a conflict: the uniqueness rule only decides calls
/// whose name is modelled.
#[tokio::test]
async fn a_name_past_the_published_maximum_length_is_refused_as_invalid() {
    let emulator = Emulator::new();
    let case = "a 256-character ChannelName, one past the modelled maximum of 255";
    let contact_arn = contact(&emulator, "oncall-primary", case).await;
    let (status, body) = emulator
        .call(
            "CreateContactChannel",
            channel_request(
                &contact_arn,
                &"n".repeat(256),
                "EMAIL",
                "ops@example.com",
                None,
            ),
        )
        .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected an over-long name to answer 400, got {status} {body:?}"
    );
    assert!(
        error_code(&body).contains("ValidationException"),
        "case {case:?}: expected a ValidationException, got {body:?}"
    );
}
