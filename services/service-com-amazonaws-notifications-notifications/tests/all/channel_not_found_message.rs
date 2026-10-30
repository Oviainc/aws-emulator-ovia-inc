// SPDX-License-Identifier: CC0-1.0

//! How this service WORDS the absence of a Channel, driven over HTTP through the
//! same multi-service router the emulator boots in production.
//!
//! The rule under test: a request naming a resource this service does not hold is
//! refused with the modelled `ResourceNotFoundException` (HTTP 404) whose message
//! is the service's ONE not-found sentence —
//! `<resource> with <identifier> <value> not found` — naming the resource that
//! was looked up, the member that identified it and the value the request carried
//! there, and nothing else. `DisassociateChannel` looks up a CHANNEL of the
//! configuration it names, so its refusal names the channel and the `arn` member
//! the channel travelled in.
//!
//! The rows vary along what the rule generalises over: why the channel is absent
//! (never associated with anything, associated with another configuration, an
//! association already removed), which of the three services the modelled
//! `ChannelArn` pattern admits the channel belongs to, how long the partition is,
//! whether the Region segment is present or empty, and how deep the resource path
//! is — plus the other resource a channel operation can fail to find (the
//! configuration), so the two refusals of one operation are told apart. Catalog
//! source:
//! `smithy_query(query="operation",operation="DisassociateChannel,GetManagedNotificationConfiguration",model_path="services/service-com-amazonaws-notifications-notifications/smithy-model/com-amazonaws-notifications-notifications.json")`
//! — `arn` `@pattern("^arn:[a-z-]{3,10}:(chatbot|consoleapp|notifications-contacts):[a-zA-Z0-9-]*:[0-9]{12}:[a-zA-Z0-9-_.@]+/[a-zA-Z0-9/_.@:-]+$")`
//! and the `ResourceNotFoundException` each declares with `httpError` 404.

use serde_json::{json, Value};

// The HTTP harness this suite drives the service through, shared with every
// other suite in this crate.
use crate::harness::{encode_path_label, error_code, Emulator};

/// A well-formed `NotificationConfigurationArn` this emulator never minted, so
/// the refusal comes from the handler rather than from the boundary validator.
const UNMINTED_CONFIGURATION_ARN: &str =
    "arn:aws:notifications::123456789012:configuration/abcdefghijklmnopqrstuvwxyz0";

/// A well-formed `ManagedNotificationConfigurationOsArn` this service publishes
/// nothing under. Catalog source: the pattern the model declares for
/// `GetManagedNotificationConfiguration`'s `arn`,
/// `^arn:[a-z-]{3,10}:notifications::[0-9]{12}:managed-notification-configuration/category/[a-zA-Z0-9\-]{3,64}/sub-category/[a-zA-Z0-9\-]{3,64}$`.
const UNPUBLISHED_MANAGED_ARN: &str = "arn:aws:notifications::123456789012:managed-notification-configuration/category/NoSuchCategory/sub-category/NoSuchSubCategory";

/// The channels the rows draw from. Catalog source: the `ChannelArn` pattern
/// above — all three services the alternation names, the shortest and the longest
/// partition the `{3,10}` bound admits, a present and an empty Region segment,
/// and a one-level and a multi-level resource path. None of them is the channel
/// any issue cited.
const CHANNELS: &[(&str, &str)] = &[
    (
        "a chatbot channel in a Region",
        "arn:aws:chatbot:us-west-2:123456789012:chat-configuration/slack-channel/duty-room",
    ),
    (
        "the Console Mobile Application, whose arn carries no Region",
        "arn:aws:consoleapp::123456789012:device/4d1f5a0e9c2b4e7f8a3c6d9b0e1f2a3b",
    ),
    (
        "an email contact, whose resource segment carries . and @",
        "arn:aws:notifications-contacts::123456789012:emailcontact/on.call@example.com",
    ),
    (
        "the longest partition the pattern admits, at ten characters",
        "arn:aws-us-gov:chatbot:us-gov-east-1:123456789012:chat-configuration/team/alarms",
    ),
    (
        "a deep resource path carrying _ and :",
        "arn:aws:chatbot:eu-central-1:123456789012:chat-configuration/teams_channel/tenant:7/room_3",
    ),
];

/// The whole sentence a not-found refusal must state, spelled here rather than
/// read from the service so the test asserts the wording instead of echoing it.
fn not_found_message(resource: &str, identifier: &str, value: &str) -> String {
    format!("{resource} with {identifier} {value} not found")
}

/// Create a NotificationConfiguration channels are associated with, and return
/// its arn.
async fn create_association_host(emulator: &Emulator, name: &str) -> String {
    let (status, response) = emulator
        .send(
            "POST",
            "/notification-configurations",
            Some(json!({ "name": name, "description": "a configuration under test" })),
        )
        .await;
    assert_eq!(
        status, 201,
        "case {name:?}: the configuration under test must be created, got {response:?}"
    );
    response["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name:?}: a create must answer an arn, got {response:?}"))
        .to_string()
}

async fn associate(emulator: &Emulator, channel: &str, configuration: &str) -> (u16, Value) {
    let uri = format!("/channels/associate/{}", encode_path_label(channel));
    emulator
        .send(
            "POST",
            &uri,
            Some(json!({ "notificationConfigurationArn": configuration })),
        )
        .await
}

async fn disassociate(emulator: &Emulator, channel: &str, configuration: &str) -> (u16, Value) {
    let uri = format!("/channels/disassociate/{}", encode_path_label(channel));
    emulator
        .send(
            "POST",
            &uri,
            Some(json!({ "notificationConfigurationArn": configuration })),
        )
        .await
}

/// The channels one configuration reports.
async fn listed_channels(emulator: &Emulator, case: &str, configuration: &str) -> Vec<String> {
    let uri = format!(
        "/channels?notificationConfigurationArn={}&maxResults=100",
        encode_path_label(configuration)
    );
    let (status, listed) = emulator.send("GET", &uri, None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: a channel listing answers 200, got {listed:?}"
    );
    listed["channels"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: a listing answers a list, got {listed:?}"))
        .iter()
        .map(|entry| entry.as_str().unwrap_or_default().to_string())
        .collect()
}

/// RULE: disassociating a channel the named configuration does not carry
/// references a resource that does not exist, so it is refused with the modelled
/// `ResourceNotFoundException` (HTTP 404) whose message states the CHANNEL that
/// was looked up and the `arn` it was named by — the same sentence every
/// not-found refusal of this service states — and changes nothing.
#[tokio::test]
async fn disassociating_a_channel_that_is_not_associated_states_the_channel_as_not_found() {
    for (channel_case, channel) in CHANNELS {
        // Rows differ in WHY the configuration does not carry the channel.
        for absence in [
            "never associated with anything",
            "associated with another configuration",
            "an association already removed",
        ] {
            let case = format!("{channel_case}, {absence}");
            let emulator = Emulator::new();
            let configuration = create_association_host(&emulator, "association-host").await;

            // One association that must survive the refusal, so the refusal is
            // about the channel under test rather than about an empty
            // configuration.
            let (_, held_channel) = CHANNELS[0];
            let held = if held_channel == *channel {
                CHANNELS[1].1
            } else {
                held_channel
            };
            let (status, associated) = associate(&emulator, held, &configuration).await;
            assert_eq!(
                status, 201,
                "case {case:?}: the association that must survive is made, got {associated:?}"
            );

            match absence {
                "associated with another configuration" => {
                    let elsewhere = create_association_host(&emulator, "other-host").await;
                    let (status, associated) = associate(&emulator, channel, &elsewhere).await;
                    assert_eq!(
                        status, 201,
                        "case {case:?}: the association elsewhere is made, got {associated:?}"
                    );
                }
                "an association already removed" => {
                    let (status, associated) = associate(&emulator, channel, &configuration).await;
                    assert_eq!(
                        status, 201,
                        "case {case:?}: the association later removed is made, got {associated:?}"
                    );
                    let (status, removed) = disassociate(&emulator, channel, &configuration).await;
                    assert_eq!(
                        status, 200,
                        "case {case:?}: removing that association answers 200, got {removed:?}"
                    );
                }
                _ => {}
            }

            let (status, refused) = disassociate(&emulator, channel, &configuration).await;
            assert_eq!(
                status, 404,
                "case {case:?}: a disassociation with nothing to remove answers the modelled 404, got {refused:?}"
            );
            assert!(
                error_code(&refused).contains("ResourceNotFoundException"),
                "case {case:?}: that refusal is a ResourceNotFoundException, got {refused:?}"
            );
            let stated = refused["message"].as_str().unwrap_or_default().to_string();
            let expected = not_found_message("channel", "arn", channel);
            assert_eq!(
                stated, expected,
                "case {case:?}: expected message {expected:?}, got {stated:?}"
            );
            assert!(
                !stated.contains(&configuration),
                "case {case:?}: the message names the channel that was looked up and no other resource, got {stated:?}"
            );
            assert_eq!(
                refused["resourceId"].as_str().unwrap_or_default(),
                *channel,
                "case {case:?}: the refusal names the channel as the resource, got {refused:?}"
            );
            assert_eq!(
                listed_channels(&emulator, &case, &configuration).await,
                vec![held.to_string()],
                "case {case:?}: a refused disassociation changes nothing"
            );

            // The same operation SUCCEEDS for a channel the configuration does
            // carry, so the refusal states an absence rather than a rejection of
            // the arn's shape.
            let (status, removed) = disassociate(&emulator, held, &configuration).await;
            assert_eq!(
                status, 200,
                "case {case:?}: disassociating an associated channel still answers 200, got {removed:?}"
            );
            assert!(
                listed_channels(&emulator, &case, &configuration)
                    .await
                    .is_empty(),
                "case {case:?}: the association that succeeded is gone"
            );
        }
    }
}

/// RULE: the sentence names the resource the request FAILED to find, so the two
/// resources a channel operation looks up — the configuration it names and the
/// channel it carries — are stated apart, and the same sentence covers the other
/// arn-keyed resources of this service.
#[tokio::test]
async fn each_absent_resource_is_stated_as_the_resource_that_was_looked_up() {
    let emulator = Emulator::new();
    let (_, channel) = CHANNELS[0];

    // A configuration that existed and was deleted, so a row's absence is not
    // only "never minted".
    let deleted = create_association_host(&emulator, "to-be-deleted").await;
    let (status, associated) = associate(&emulator, channel, &deleted).await;
    assert_eq!(status, 201, "the association before the delete is made, got {associated:?}");
    let (status, dropped) = emulator
        .send(
            "DELETE",
            &format!("/notification-configurations/{}", encode_path_label(&deleted)),
            None,
        )
        .await;
    assert_eq!(status, 200, "the configuration is deleted, got {dropped:?}");

    // Each row: the request, the resource it failed to find, the member that
    // identified it and the value it carried there.
    for (case, configuration, resource, value) in [
        (
            "a configuration arn this emulator never minted",
            UNMINTED_CONFIGURATION_ARN.to_string(),
            "notification configuration",
            UNMINTED_CONFIGURATION_ARN.to_string(),
        ),
        (
            "a configuration that was deleted",
            deleted.clone(),
            "notification configuration",
            deleted.clone(),
        ),
    ] {
        let (status, refused) = disassociate(&emulator, channel, &configuration).await;
        assert_eq!(
            status, 404,
            "case {case:?}: the lookup answers the modelled 404, got {refused:?}"
        );
        let stated = refused["message"].as_str().unwrap_or_default().to_string();
        let expected = not_found_message(resource, "arn", &value);
        assert_eq!(
            stated, expected,
            "case {case:?}: expected message {expected:?}, got {stated:?}"
        );
    }

    // The other arn-keyed resource a read looks up states the same sentence.
    let (status, refused) = emulator
        .send(
            "GET",
            &format!(
                "/managed-notification-configurations/{}",
                encode_path_label(UNPUBLISHED_MANAGED_ARN)
            ),
            None,
        )
        .await;
    assert_eq!(
        status, 404,
        "case a managed configuration nothing is published under: the lookup answers the modelled 404, got {refused:?}"
    );
    assert!(
        error_code(&refused).contains("ResourceNotFoundException"),
        "case a managed configuration nothing is published under: that refusal is a ResourceNotFoundException, got {refused:?}"
    );
    let stated = refused["message"].as_str().unwrap_or_default().to_string();
    let expected = not_found_message(
        "managed notification configuration",
        "arn",
        UNPUBLISHED_MANAGED_ARN,
    );
    assert_eq!(
        stated, expected,
        "case a managed configuration nothing is published under: expected message {expected:?}, got {stated:?}"
    );
}
