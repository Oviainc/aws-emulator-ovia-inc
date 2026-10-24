// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Channel association lifecycle: `AssociateChannel`,
//! `ListChannels` and the `DisassociateChannel` that undoes an association,
//! driven over HTTP through the same multi-service router the emulator boots in
//! production.
//!
//! Every case asserts the RULE the operations follow, not one recorded request.
//! The table rows vary along what those rules generalise over: which of the three
//! services the modelled `ChannelArn` pattern admits the channel belongs to, how
//! long the partition segment is, whether the Region segment is present or empty,
//! how deep the resource path is and which of the punctuation characters the
//! pattern permits it carries. The catalogs the rows draw from are the model's
//! own constraints (`ChannelArn`'s `@pattern`, `maxResults`'s `@range`) and this
//! service's own bound (`MAX_CHANNELS_PER_CONFIGURATION`), so a model change
//! moves the test with it.

use serde_json::{json, Value};
use service_com_amazonaws_notifications_notifications::state::MAX_CHANNELS_PER_CONFIGURATION;
use service_com_amazonaws_notifications_notifications::ComAmazonawsNotificationsNotificationsState;

// The HTTP harness this suite drives the service through, shared with every
// other suite in this crate.
use crate::harness::{encode_path_label, error_code, Emulator};

/// A well-formed `NotificationConfigurationArn` this emulator never minted — the
/// arn pattern is satisfied, so a refusal comes from the handler rather than from
/// the boundary validator.
const UNMINTED_CONFIGURATION_ARN: &str =
    "arn:aws:notifications::123456789012:configuration/abcdefghijklmnopqrstuvwxyz0";

/// The channels the rows draw from.
///
/// Catalog source: the `ChannelArn` shape the model declares for both
/// association operations' `arn` and for the members of `ListChannels`'s
/// `channels` —
/// `^arn:[a-z-]{3,10}:(chatbot|consoleapp|notifications-contacts):[a-zA-Z0-9-]*:[0-9]{12}:[a-zA-Z0-9-_.@]+/[a-zA-Z0-9/_.@:-]+$`.
/// The rows cover all three services the alternation names, the shortest and the
/// longest partition the `{3,10}` bound admits, a present and an empty Region
/// segment, a one-level and a multi-level resource path, and the `.`, `_`, `@`
/// and `:` characters the resource segments permit — including channels no issue
/// cited.
const CHANNELS: &[(&str, &str)] = &[
    (
        "a chatbot channel in a Region",
        "arn:aws:chatbot:us-east-1:123456789012:chat-configuration/slack-channel/ops-room",
    ),
    (
        "the Console Mobile Application, whose arn carries no Region",
        "arn:aws:consoleapp::123456789012:device/0bfe9a191c084b9fae82a53b4c1d4cd1",
    ),
    (
        "an email contact, whose resource segment carries . and @",
        "arn:aws:notifications-contacts::123456789012:emailcontact/first.last@example.com",
    ),
    (
        "the longest partition the pattern admits, at ten characters",
        "arn:aws-us-gov:chatbot:us-gov-west-1:123456789012:chat-configuration/team-channel/alerts",
    ),
    (
        "a deep resource path carrying _ and :",
        "arn:aws:chatbot:eu-west-1:123456789012:chat-configuration/teams_channel/tenant:1/room_2",
    ),
];

/// The channel-specific helpers this suite adds to the shared harness.
impl Emulator {
    /// Create a NotificationConfiguration for a channel to be associated with,
    /// and return its arn.
    async fn create_channel_host(&self, name: &str) -> String {
        let (status, response) = self
            .send(
                "POST",
                "/notification-configurations",
                Some(json!({ "name": name, "description": "a configuration under test" })),
            )
            .await;
        assert_eq!(
            status, 201,
            "the configuration a channel is associated with must be created, got {response:?}"
        );
        response["arn"]
            .as_str()
            .unwrap_or_else(|| panic!("a create must answer an arn, got {response:?}"))
            .to_string()
    }

    async fn associate_channel(&self, channel: &str, configuration: &str) -> (u16, Value) {
        let uri = format!("/channels/associate/{}", encode_path_label(channel));
        self.send(
            "POST",
            &uri,
            Some(json!({ "notificationConfigurationArn": configuration })),
        )
        .await
    }

    async fn disassociate_channel(&self, channel: &str, configuration: &str) -> (u16, Value) {
        let uri = format!("/channels/disassociate/{}", encode_path_label(channel));
        self.send(
            "POST",
            &uri,
            Some(json!({ "notificationConfigurationArn": configuration })),
        )
        .await
    }

    /// One page of `ListChannels`, with whatever extra query members the case
    /// needs appended.
    async fn list_channels_raw(&self, configuration: &str, extra: &str) -> (u16, Value) {
        let mut uri = format!("/channels?notificationConfigurationArn={configuration}");
        if !extra.is_empty() {
            uri.push('&');
            uri.push_str(extra);
        }
        self.send("GET", &uri, None).await
    }

    /// Every channel the default page of the listing reports, which must answer
    /// 200.
    async fn list_channels(&self, case: &str, configuration: &str) -> Vec<String> {
        let (status, listed) = self.list_channels_raw(configuration, "").await;
        assert_eq!(
            status, 200,
            "case {case:?}: a listing of an existing configuration answers 200, got {listed:?}"
        );
        channels_of(case, &listed)
    }

    /// How many channels the store holds for one configuration, read straight
    /// out of it — what a refused call must not have changed.
    fn stored_channel_count(&self, configuration: &str) -> usize {
        let state = self
            .engine
            .state::<ComAmazonawsNotificationsNotificationsState>();
        let store = state.lock();
        store
            .configurations
            .get(configuration)
            .map(|record| record.channel_arns.len())
            .unwrap_or_default()
    }
}

/// The required `channels` member of a listing response, as a list of arns.
fn channels_of(case: &str, listed: &Value) -> Vec<String> {
    let channels = listed["channels"].as_array().unwrap_or_else(|| {
        panic!("case {case:?}: every listing carries the required channels member, got {listed:?}")
    });
    channels
        .iter()
        .map(|channel| {
            channel
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: a listed channel is an arn string, got {channel:?}")
                })
                .to_string()
        })
        .collect()
}

/// Whether a response carries no members at all, which is what both association
/// operations' modelled empty output renders as.
fn is_empty_body(response: &Value) -> bool {
    *response == Value::Null || response.as_object().is_some_and(serde_json::Map::is_empty)
}

/// A channel arn built from a row index, used where a case needs more distinct
/// channels than the table lists. Satisfies the `ChannelArn` pattern for every
/// index.
fn generated_channel(index: usize) -> String {
    format!("arn:aws:chatbot:us-east-1:123456789012:chat-configuration/slack-channel/room-{index}")
}

/// RULE: associating a channel with a NotificationConfiguration persists exactly
/// that association, which the configuration's channel listing then reports and
/// `ListNotificationConfigurations`'s `channelArn` filter selects the
/// configuration on; the operation is `@idempotent`, so repeating it answers the
/// same 201 and leaves one association, not two.
#[tokio::test]
async fn associate_persists_an_association_the_listing_reports_and_a_repeat_changes_nothing() {
    for (case, channel) in CHANNELS {
        let emulator = Emulator::new();
        let configuration = emulator.create_channel_host("channel-host").await;

        let empty = emulator.list_channels(case, &configuration).await;
        assert!(
            empty.is_empty(),
            "case {case:?}: a configuration starts with no channel associated, got {empty:?}"
        );

        let (status, associated) = emulator.associate_channel(channel, &configuration).await;
        assert_eq!(
            status, 201,
            "case {case:?}: an association answers the modelled 201, got {associated:?}"
        );
        assert!(
            is_empty_body(&associated),
            "case {case:?}: the modelled response carries no members, got {associated:?}"
        );

        let listed = emulator.list_channels(case, &configuration).await;
        assert_eq!(
            listed,
            vec![(*channel).to_string()],
            "case {case:?}: the listing reports exactly the associated channel, got {listed:?}"
        );

        // The association is also what the configuration listing's `channelArn`
        // filter selects a configuration on.
        let (status, filtered) = emulator
            .send(
                "GET",
                &format!("/notification-configurations?channelArn={channel}"),
                None,
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: filtering configurations by channel answers 200, got {filtered:?}"
        );
        let selected: Vec<&str> = filtered["notificationConfigurations"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: the filtered listing is a list, got {filtered:?}"))
            .iter()
            .filter_map(|item| item["arn"].as_str())
            .collect();
        assert_eq!(
            selected,
            vec![configuration.as_str()],
            "case {case:?}: the channel filter selects the associated configuration, got {selected:?}"
        );

        let (status, repeated) = emulator.associate_channel(channel, &configuration).await;
        assert_eq!(
            status, 201,
            "case {case:?}: a repeat of the same association succeeds, got {repeated:?}"
        );
        let listed = emulator.list_channels(case, &configuration).await;
        assert_eq!(
            listed,
            vec![(*channel).to_string()],
            "case {case:?}: a repeat leaves ONE association, got {listed:?}"
        );
        assert_eq!(
            emulator.stored_channel_count(&configuration),
            1,
            "case {case:?}: a repeat stores no second entry, got {} entries",
            emulator.stored_channel_count(&configuration)
        );
    }
}

/// RULE: a configuration carries every channel associated with it, in the order
/// they were associated, and each association is independent of the others — so
/// one configuration's associations say nothing about another's.
#[tokio::test]
async fn every_association_is_reported_in_association_order_and_per_configuration() {
    let emulator = Emulator::new();
    let first = emulator.create_channel_host("first-host").await;
    let second = emulator.create_channel_host("second-host").await;

    let mut expected: Vec<String> = Vec::new();
    for (case, channel) in CHANNELS {
        let (status, associated) = emulator.associate_channel(channel, &first).await;
        assert_eq!(
            status, 201,
            "case {case:?}: an association answers 201, got {associated:?}"
        );
        expected.push((*channel).to_string());

        let listed = emulator.list_channels(case, &first).await;
        assert_eq!(
            listed, expected,
            "case {case:?}: the listing reports every association in association order, got {listed:?}"
        );
        let other = emulator.list_channels(case, &second).await;
        assert!(
            other.is_empty(),
            "case {case:?}: associating with one configuration must not touch another, got {other:?}"
        );
    }
}

/// RULE: disassociating removes exactly the named association and leaves every
/// other one standing, and the channel can be associated again afterwards — a
/// re-association lands after the associations that outlived it, because order is
/// the order the associations were made.
#[tokio::test]
async fn disassociate_removes_only_the_named_association_and_allows_reassociation() {
    // Each row in turn is the one removed, so no single position (first, last or
    // in the middle) is special.
    for (removed_index, (case, removed)) in CHANNELS.iter().enumerate() {
        let emulator = Emulator::new();
        let configuration = emulator.create_channel_host("channel-host").await;
        for (_, channel) in CHANNELS {
            let (status, associated) = emulator.associate_channel(channel, &configuration).await;
            assert_eq!(
                status, 201,
                "case {case:?}: the setup association answers 201, got {associated:?}"
            );
        }

        let (status, disassociated) = emulator.disassociate_channel(removed, &configuration).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a disassociation answers the modelled 200, got {disassociated:?}"
        );
        assert!(
            is_empty_body(&disassociated),
            "case {case:?}: the modelled response carries no members, got {disassociated:?}"
        );

        let survivors: Vec<String> = CHANNELS
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != removed_index)
            .map(|(_, (_, channel))| (*channel).to_string())
            .collect();
        let listed = emulator.list_channels(case, &configuration).await;
        assert_eq!(
            listed, survivors,
            "case {case:?}: only the named association is removed, got {listed:?}"
        );

        // Associating again is permitted, and the channel is listed once more —
        // at the end, since it is the most recent association.
        let (status, again) = emulator.associate_channel(removed, &configuration).await;
        assert_eq!(
            status, 201,
            "case {case:?}: a disassociated channel can be associated again, got {again:?}"
        );
        let mut reassociated = survivors.clone();
        reassociated.push((*removed).to_string());
        let listed = emulator.list_channels(case, &configuration).await;
        assert_eq!(
            listed, reassociated,
            "case {case:?}: the re-associated channel is listed again, got {listed:?}"
        );
    }
}

/// RULE: there is only something to disassociate where an association exists, so
/// disassociating a channel a configuration does not carry references a resource
/// that does not exist — the modelled `ResourceNotFoundException` (HTTP 404) —
/// and changes nothing.
#[tokio::test]
async fn disassociating_a_channel_that_is_not_associated_is_refused() {
    let emulator = Emulator::new();
    let configuration = emulator.create_channel_host("channel-host").await;
    let elsewhere = emulator.create_channel_host("other-host").await;

    // The association that must survive every refusal below.
    let (held_case, held) = CHANNELS[0];
    let (status, _) = emulator.associate_channel(held, &configuration).await;
    assert_eq!(status, 201, "case {held_case:?}: the setup association answers 201");
    // Associated with ANOTHER configuration, which says nothing about this one.
    let (_, other_channel) = CHANNELS[1];
    let (status, _) = emulator.associate_channel(other_channel, &elsewhere).await;
    assert_eq!(status, 201, "the setup association on the other configuration answers 201");
    // Associated and then removed, so the association existed once.
    let (_, removed) = CHANNELS[2];
    let (status, _) = emulator.associate_channel(removed, &configuration).await;
    assert_eq!(status, 201, "the association that is then removed answers 201");
    let (status, _) = emulator.disassociate_channel(removed, &configuration).await;
    assert_eq!(status, 200, "removing that association answers 200");

    let cases = [
        ("a channel never associated with anything", CHANNELS[3].1),
        ("a channel associated with another configuration", other_channel),
        ("a channel whose association was already removed", removed),
    ];
    for (case, channel) in cases {
        let (status, refused) = emulator.disassociate_channel(channel, &configuration).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a disassociation with nothing to remove is refused with 404, got {refused:?}"
        );
        assert!(
            error_code(&refused).contains("ResourceNotFoundException"),
            "case {case:?}: that refusal is a ResourceNotFoundException, got {refused:?}"
        );
        let listed = emulator.list_channels(case, &configuration).await;
        assert_eq!(
            listed,
            vec![held.to_string()],
            "case {case:?}: a refused disassociation changes nothing, got {listed:?}"
        );
    }
}

/// RULE: every channel operation is scoped to a NotificationConfiguration, so an
/// arn naming none — never minted, or deleted since — references a resource that
/// does not exist and is refused with the modelled `ResourceNotFoundException`
/// (HTTP 404), never with an empty success.
#[tokio::test]
async fn a_configuration_that_does_not_exist_is_refused_by_every_channel_operation() {
    let emulator = Emulator::new();

    // A configuration that existed and was deleted, so the refusal is about the
    // record being gone rather than about the arn never having been minted.
    let deleted = emulator.create_channel_host("to-be-deleted").await;
    let (_, channel) = CHANNELS[0];
    let (status, _) = emulator.associate_channel(channel, &deleted).await;
    assert_eq!(status, 201, "the association made before the delete answers 201");
    let (status, _) = emulator
        .send(
            "DELETE",
            &format!("/notification-configurations/{}", encode_path_label(&deleted)),
            None,
        )
        .await;
    assert_eq!(status, 200, "the configuration is deleted");

    for (case, configuration) in [
        ("a configuration arn this emulator never minted", UNMINTED_CONFIGURATION_ARN),
        ("a configuration that was deleted", deleted.as_str()),
    ] {
        let (status, refused) = emulator.associate_channel(channel, configuration).await;
        assert_eq!(
            status, 404,
            "case {case:?}: AssociateChannel is refused with 404, got {refused:?}"
        );
        assert!(
            error_code(&refused).contains("ResourceNotFoundException"),
            "case {case:?}: AssociateChannel's refusal is a ResourceNotFoundException, got {refused:?}"
        );

        let (status, refused) = emulator.disassociate_channel(channel, configuration).await;
        assert_eq!(
            status, 404,
            "case {case:?}: DisassociateChannel is refused with 404, got {refused:?}"
        );
        assert!(
            error_code(&refused).contains("ResourceNotFoundException"),
            "case {case:?}: DisassociateChannel's refusal is a ResourceNotFoundException, got {refused:?}"
        );

        let (status, refused) = emulator.list_channels_raw(configuration, "").await;
        assert_eq!(
            status, 404,
            "case {case:?}: ListChannels is refused with 404, got {refused:?}"
        );
        assert!(
            error_code(&refused).contains("ResourceNotFoundException"),
            "case {case:?}: ListChannels' refusal is a ResourceNotFoundException, got {refused:?}"
        );
    }
}

/// RULE: the channel listing is cut into pages of at most `maxResults`, and a
/// page token resumes exactly where the previous page stopped — so the pages
/// concatenate to every association in association order, whatever the page size,
/// and only a page with associations behind it carries a token.
#[tokio::test]
async fn the_listing_pages_by_max_results_and_resumes_where_it_stopped() {
    let emulator = Emulator::new();
    let configuration = emulator.create_channel_host("channel-host").await;

    // More channels than the smallest page sizes below, so several rows page more
    // than twice.
    let associated: Vec<String> = (0..7).map(generated_channel).collect();
    for channel in &associated {
        let (status, response) = emulator.associate_channel(channel, &configuration).await;
        assert_eq!(status, 201, "the setup association answers 201, got {response:?}");
    }

    // Page sizes spanning the published `maxResults` range: its minimum (1), a
    // size that divides the set, one that does not, exactly the set size, and the
    // maximum (100) — the bounds come from the member's own `@range(1..100)`.
    for page_size in [1, 2, 3, 7, 100] {
        let case = format!("maxResults={page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            pages += 1;
            assert!(
                pages <= associated.len() + 2,
                "case {case:?}: paging did not terminate after {pages} pages"
            );
            let extra = match &token {
                Some(value) => format!("maxResults={page_size}&nextToken={value}"),
                None => format!("maxResults={page_size}"),
            };
            let (status, page) = emulator.list_channels_raw(&configuration, &extra).await;
            assert_eq!(
                status, 200,
                "case {case:?}: a page answers 200, got {page:?}"
            );
            let channels = channels_of(&case, &page);
            assert!(
                channels.len() <= page_size,
                "case {case:?}: a page must not exceed maxResults, got {} entries",
                channels.len()
            );
            seen.extend(channels.iter().cloned());
            match page.get("nextToken").and_then(Value::as_str) {
                Some(next) => {
                    assert!(
                        seen.len() < associated.len(),
                        "case {case:?}: a token was emitted after the last association"
                    );
                    assert!(
                        !channels.is_empty(),
                        "case {case:?}: an empty page must not carry a token"
                    );
                    token = Some(next.to_string());
                }
                None => break,
            }
        }
        assert_eq!(
            seen, associated,
            "case {case:?}: the pages concatenate to every association in association order, got {seen:?}"
        );
    }
}

/// RULE: a `nextToken` this service never issued is refused with the modelled
/// `ValidationException` (HTTP 400) rather than silently restarting the listing
/// at the first association.
#[tokio::test]
async fn a_forged_next_token_is_refused() {
    let emulator = Emulator::new();
    let configuration = emulator.create_channel_host("channel-host").await;
    let (_, channel) = CHANNELS[0];
    let (status, _) = emulator.associate_channel(channel, &configuration).await;
    assert_eq!(status, 201, "the setup association answers 201");

    // Each satisfies the modelled `NextToken` constraints (`@length(1..4096)`,
    // `^[\w+-/=]+$`), so the boundary validator passes them to the handler: what
    // refuses them is that this service never issued them.
    for (case, token) in [
        ("a token of the right length that is not hex", "z".repeat(32)),
        ("a token of hex at the wrong length", "ab".to_string()),
        ("well-formed hex carrying another tag", "0".repeat(32)),
    ] {
        let (status, refused) = emulator
            .list_channels_raw(&configuration, &format!("nextToken={token}"))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a forged nextToken is refused with 400, got {refused:?}"
        );
        assert!(
            error_code(&refused).contains("ValidationException"),
            "case {case:?}: that refusal is a ValidationException, got {refused:?}"
        );
    }
}

/// RULE: the stored set of a configuration's associations is bounded, so an
/// association past the ceiling is refused with the error the model declares for
/// a quota (`ServiceQuotaExceededException`, HTTP 402) and the store does not
/// grow — while a repeat of an association the configuration already carries
/// still succeeds at the ceiling, because an idempotent repeat consumes no quota.
#[tokio::test]
async fn an_association_past_the_bound_is_refused_and_a_repeat_at_the_bound_is_not() {
    let emulator = Emulator::new();
    let configuration = emulator.create_channel_host("channel-host").await;

    // Catalog source: `MAX_CHANNELS_PER_CONFIGURATION`, this service's own bound,
    // re-exported by the service crate.
    for index in 0..MAX_CHANNELS_PER_CONFIGURATION {
        let channel = generated_channel(index);
        let (status, response) = emulator.associate_channel(&channel, &configuration).await;
        assert_eq!(
            status, 201,
            "association {index} of the bound must be served, got {response:?}"
        );
    }
    assert_eq!(
        emulator.stored_channel_count(&configuration),
        MAX_CHANNELS_PER_CONFIGURATION,
        "the configuration holds the bound's worth of associations, got {}",
        emulator.stored_channel_count(&configuration)
    );

    let past_the_bound = generated_channel(MAX_CHANNELS_PER_CONFIGURATION);
    let (status, refused) = emulator
        .associate_channel(&past_the_bound, &configuration)
        .await;
    assert_eq!(
        status, 402,
        "case past the bound: the association is refused with the modelled 402, got {refused:?}"
    );
    assert!(
        error_code(&refused).contains("ServiceQuotaExceededException"),
        "case past the bound: that refusal is a ServiceQuotaExceededException, got {refused:?}"
    );
    assert_eq!(
        emulator.stored_channel_count(&configuration),
        MAX_CHANNELS_PER_CONFIGURATION,
        "case past the bound: a refused association must not grow the store, got {}",
        emulator.stored_channel_count(&configuration)
    );

    let (status, repeated) = emulator
        .associate_channel(&generated_channel(0), &configuration)
        .await;
    assert_eq!(
        status, 201,
        "case a repeat at the bound: an idempotent repeat consumes no quota, got {repeated:?}"
    );

    // Freeing one slot admits one new association, so the bound is about how many
    // associations are held and not about how many requests were made.
    let (status, _) = emulator
        .disassociate_channel(&generated_channel(0), &configuration)
        .await;
    assert_eq!(status, 200, "case after a disassociation: the removal answers 200");
    let (status, response) = emulator
        .associate_channel(&past_the_bound, &configuration)
        .await;
    assert_eq!(
        status, 201,
        "case after a disassociation: the freed slot admits one association, got {response:?}"
    );
}
