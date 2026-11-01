// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ListChannels`, driven over HTTP through the same router the
//! emulator boots, reusing the harness in [`crate::channel_lifecycle`].
//!
//! The rule under test: `ListChannels` reports exactly the channels persisted under the
//! addressed channel group, in name order, at most `MaxResults` per page (10 when the
//! member is absent) with a `NextToken` issued only while channels remain, each item
//! being the list projection of what the create persisted; a group that does not resolve
//! answers `ResourceNotFoundException` (404) and a token the service did not issue
//! answers `ValidationException` (400). Each test asserts that over a SET of inputs.
//!
//! The last test pins a second rule the same lifecycle depends on: a request naming a
//! channel that does not exist is refused with the service's OWN fixed channel-not-found
//! sentence, identically from every operation that resolves a channel.

use serde_json::{json, Value};

use crate::channel_lifecycle::{error_type, Emulator};

/// Walk every page of `ListChannels` for one group, returning the channel names in the
/// order they were reported. Asserts the per-page invariants as it goes: no page is
/// longer than the effective page size, no item belongs to another group, and a token is
/// present only while something is left to report.
async fn drain_pages(
    emulator: &Emulator,
    group: &str,
    max_results: Option<i32>,
    label: &str,
) -> Vec<String> {
    // The page size the model declares as `MaxResults`'s default.
    let page_cap = max_results.unwrap_or(10) as usize;
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let mut query: Vec<String> = Vec::new();
        if let Some(limit) = max_results {
            query.push(format!("maxResults={limit}"));
        }
        if let Some(token) = &token {
            query.push(format!("nextToken={token}"));
        }
        let query = if query.is_empty() {
            String::new()
        } else {
            format!("?{}", query.join("&"))
        };

        let (status, body) = emulator.list_channels(group, &query).await;
        assert_eq!(
            status, 200,
            "case {label:?}: ListChannels expected 200, got {status} {body}"
        );
        let items = body["Items"].as_array().cloned().unwrap_or_default();
        assert!(
            items.len() <= page_cap,
            "case {label:?}: a page carried {} items, more than maxResults {page_cap}",
            items.len()
        );
        for item in &items {
            assert_eq!(
                item["ChannelGroupName"],
                json!(group),
                "case {label:?}: the list must only report this group's channels, got {item}"
            );
            seen.push(
                item["ChannelName"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {label:?}: an item without a name: {item}"))
                    .to_string(),
            );
        }

        token = body["NextToken"].as_str().map(str::to_string);
        if token.is_some() {
            assert_eq!(
                items.len(),
                page_cap,
                "case {label:?}: a token was issued after a short page of {} items",
                items.len()
            );
        }
        pages += 1;
        assert!(
            pages <= page_cap.max(1) + seen.len() + 2,
            "case {label:?}: paging did not terminate after {pages} pages"
        );
        if token.is_none() {
            break seen;
        }
    }
}

/// Paging covers every channel of the addressed group exactly once, in name order.
///
/// The rows vary the number of channels and the page size, including both published
/// bounds of the model's `MaxResults` range (1 and 100) and the absent member (which
/// takes the modelled default of 10).
#[tokio::test]
async fn listing_pages_over_every_channel_of_the_group_exactly_once() {
    let cases: &[(usize, Option<i32>)] = &[
        (0, Some(1)),
        (1, Some(1)),
        (2, Some(1)),
        (5, Some(2)),
        (7, Some(100)),
        (12, None),
        (25, Some(10)),
    ];

    for &(count, max_results) in cases {
        let label = format!("{count} channels, maxResults={max_results:?}");
        let emulator = Emulator::new();
        assert_eq!(emulator.create_group("grp", json!({})).await.0, 200);
        // A second group with its own channels, so the list must not leak across groups.
        assert_eq!(emulator.create_group("other", json!({})).await.0, 200);
        assert_eq!(
            emulator.create_channel("other", "elsewhere", json!({})).await.0,
            200
        );

        let mut expected: Vec<String> = Vec::new();
        for index in 0..count {
            let name = format!("chan-{index:02}");
            let (status, body) = emulator.create_channel("grp", &name, json!({})).await;
            assert_eq!(
                status, 200,
                "case {label:?}: CreateChannel {name} expected 200, got {status} {body}"
            );
            expected.push(name);
        }
        expected.sort();

        let seen = drain_pages(&emulator, "grp", max_results, &label).await;
        assert_eq!(
            seen, expected,
            "case {label:?}: paging must report every channel of the group once, in name order"
        );
    }
}

/// One row of the projection table: a channel body, and the members the list must report
/// with the same values the create persisted.
struct ProjectionCase {
    label: &'static str,
    channel: &'static str,
    body: fn() -> Value,
}

/// The rows. `InputType` values come from the model's `InputType` enum (HLS, CMAF,
/// MULTIVIEW), `OutputLockingMode` from its `OutputLockingMode` enum (EPOCH_LOCKED,
/// NON_EPOCH_LOCKED) and the layout from its `MultiviewLayoutType` enum.
fn projection_cases() -> Vec<ProjectionCase> {
    vec![
        ProjectionCase {
            label: "minimal: nothing but a name",
            channel: "c-minimal",
            body: || json!({}),
        },
        ProjectionCase {
            label: "described HLS channel with tags",
            channel: "c-hls",
            body: || json!({"Description": "a channel", "InputType": "HLS", "tags": {"k": "v"}}),
        },
        ProjectionCase {
            label: "CMAF input, epoch-locked output",
            channel: "C_cmaf_0",
            body: || json!({"InputType": "CMAF", "OutputLockingMode": "EPOCH_LOCKED"}),
        },
        ProjectionCase {
            label: "CMAF input, non-epoch-locked output, empty description",
            channel: "c-cmaf-nonepoch",
            body: || {
                json!({
                    "InputType": "CMAF",
                    "OutputLockingMode": "NON_EPOCH_LOCKED",
                    "Description": "",
                })
            },
        },
        ProjectionCase {
            label: "CMAF input naming no locking mode, empty description",
            channel: "c-cmaf-default",
            body: || json!({"InputType": "CMAF", "Description": ""}),
        },
        ProjectionCase {
            label: "multiview channel compositing two sources",
            channel: "c-multiview",
            body: || {
                json!({
                    "InputType": "MULTIVIEW",
                    "MultiviewConfiguration": {
                        // A composition admits only epoch-locked sources, so the
                        // non-epoch-locked row above is not one of these.
                        "AvailableSources": ["C_cmaf_0", "c-cmaf-default"],
                        "AvailableLayouts": ["LAYOUT_2EH", "LAYOUT_4E"],
                    },
                })
            },
        },
    ]
}

/// Every member the list projection carries is the value the create persisted — the same
/// value `GetChannel` reads back for that channel.
#[tokio::test]
async fn a_listed_channel_carries_the_values_the_create_persisted() {
    /// The members `ChannelListConfiguration` shares with `GetChannelResponse`, from the
    /// model's two shapes.
    const SHARED_MEMBERS: &[&str] = &[
        "Arn",
        "ChannelName",
        "ChannelGroupName",
        "CreatedAt",
        "ModifiedAt",
        "Description",
        "InputType",
        "OutputLockingMode",
        "MultiviewConfiguration",
    ];

    let emulator = Emulator::new();
    assert_eq!(emulator.create_group("grp", json!({})).await.0, 200);
    for case in projection_cases() {
        let (status, body) = emulator
            .create_channel("grp", case.channel, (case.body)())
            .await;
        assert_eq!(
            status, 200,
            "case {:?}: CreateChannel expected 200, got {status} {body}",
            case.label
        );
    }

    let (status, listed) = emulator.list_channels("grp", "?maxResults=100").await;
    assert_eq!(status, 200, "ListChannels expected 200, got {status} {listed}");
    let items = listed["Items"].as_array().cloned().unwrap_or_default();

    for case in projection_cases() {
        let label = case.label;
        let item = items
            .iter()
            .find(|item| item["ChannelName"] == json!(case.channel))
            .unwrap_or_else(|| panic!("case {label:?}: {} is missing from {listed}", case.channel));
        let (_, read) = emulator.get_channel("grp", case.channel).await;
        for member in SHARED_MEMBERS {
            assert_eq!(
                item[member], read[member],
                "case {label:?}: listed {member} must equal what GetChannel reports"
            );
        }
        // The list form reports no ingest endpoints, ETag or tags: those are a `Get`'s.
        for member in ["IngestEndpoints", "ETag", "Tags"] {
            assert_eq!(
                item[member],
                Value::Null,
                "case {label:?}: the list projection must not carry {member}, got {item}"
            );
        }
    }
}

/// The list reflects the store's lifecycle: a created channel appears, a deleted one is
/// gone, and a re-created one appears again — with the values of the NEW resource.
#[tokio::test]
async fn the_list_reflects_every_create_and_delete() {
    let emulator = Emulator::new();
    assert_eq!(emulator.create_group("grp", json!({})).await.0, 200);

    assert_eq!(
        drain_pages(&emulator, "grp", Some(100), "empty group").await,
        Vec::<String>::new(),
        "a group with no channels must list nothing"
    );

    for name in ["alpha", "beta", "gamma"] {
        assert_eq!(
            emulator.create_channel("grp", name, json!({})).await.0,
            200,
            "CreateChannel {name} must succeed"
        );
        let seen = drain_pages(&emulator, "grp", Some(100), &format!("after create {name}")).await;
        assert!(
            seen.contains(&name.to_string()),
            "case {name:?}: a created channel must be listed, got {seen:?}"
        );
    }

    let (_, before) = emulator.list_channels("grp", "?maxResults=100").await;
    let created_beta = before["Items"]
        .as_array()
        .and_then(|items| {
            items
                .iter()
                .find(|item| item["ChannelName"] == json!("beta"))
                .cloned()
        })
        .expect("beta must be listed before it is deleted");

    assert_eq!(emulator.delete_channel("grp", "beta").await.0, 200);
    let seen = drain_pages(&emulator, "grp", Some(100), "after delete beta").await;
    assert_eq!(
        seen,
        vec!["alpha".to_string(), "gamma".to_string()],
        "a deleted channel must leave the list, got {seen:?}"
    );

    assert_eq!(
        emulator
            .create_channel("grp", "beta", json!({"Description": "the second beta"}))
            .await
            .0,
        200
    );
    let (_, after) = emulator.list_channels("grp", "?maxResults=100").await;
    let recreated_beta = after["Items"]
        .as_array()
        .and_then(|items| {
            items
                .iter()
                .find(|item| item["ChannelName"] == json!("beta"))
                .cloned()
        })
        .expect("a re-created channel must be listed again");
    assert_eq!(
        recreated_beta["Description"],
        json!("the second beta"),
        "the list must report the re-created channel, not the deleted one: {recreated_beta}"
    );
    assert_ne!(
        recreated_beta["CreatedAt"], created_beta["CreatedAt"],
        "the re-created channel is a new resource, so its CreatedAt must differ"
    );
}

/// `AttachedMultiviewChannels` is the inverse of the groups's multiview source lists: a
/// channel reports the multiview channels of its OWN group that name it as a source, and
/// omits the member when nothing does.
#[tokio::test]
async fn a_source_channel_reports_the_multiview_channels_that_attach_it() {
    let emulator = Emulator::new();
    for group in ["grp", "other"] {
        assert_eq!(emulator.create_group(group, json!({})).await.0, 200);
        for source in ["source-a", "source-b"] {
            assert_eq!(
                emulator
                    .create_channel(group, source, json!({"InputType": "CMAF"}))
                    .await
                    .0,
                200
            );
        }
    }
    assert_eq!(
        emulator.create_channel("grp", "plain", json!({})).await.0,
        200
    );
    // Two multiview channels in `grp`, and one in `other` that must not show up in it.
    for (group, name, sources) in [
        ("grp", "mv-one", vec!["source-a", "source-b"]),
        ("grp", "mv-two", vec!["source-a"]),
        ("other", "mv-other", vec!["source-a"]),
    ] {
        assert_eq!(
            emulator
                .create_channel(
                    group,
                    name,
                    json!({
                        "InputType": "MULTIVIEW",
                        "MultiviewConfiguration": {
                            "AvailableSources": sources,
                            "AvailableLayouts": ["LAYOUT_2EH"],
                        },
                    }),
                )
                .await
                .0,
            200,
            "CreateChannel {name} must succeed"
        );
    }

    let (status, body) = emulator.list_channels("grp", "?maxResults=100").await;
    assert_eq!(status, 200, "ListChannels expected 200, got {status} {body}");
    let items = body["Items"].as_array().cloned().unwrap_or_default();

    let expected: &[(&str, Value)] = &[
        // Both of `grp`'s multiview channels list `source-a`, in name order.
        ("source-a", json!(["mv-one", "mv-two"])),
        ("source-b", json!(["mv-one"])),
        // Nothing attaches these, so the member is absent.
        ("plain", Value::Null),
        ("mv-one", Value::Null),
        ("mv-two", Value::Null),
    ];
    for (channel, attached) in expected {
        let item = items
            .iter()
            .find(|item| item["ChannelName"] == json!(channel))
            .unwrap_or_else(|| panic!("case {channel:?}: missing from {body}"));
        assert_eq!(
            item["AttachedMultiviewChannels"], *attached,
            "case {channel:?}: expected AttachedMultiviewChannels {attached}, got {item}"
        );
    }
}

/// A group that does not resolve is a 404, whether it never existed or was deleted.
#[tokio::test]
async fn listing_a_group_that_does_not_resolve_is_not_found() {
    let emulator = Emulator::new();
    assert_eq!(emulator.create_group("gone", json!({})).await.0, 200);
    assert_eq!(
        emulator.call("DELETE", "/channelGroup/gone", None).await.0,
        200
    );

    for group in ["absent", "gone", "Another_Group-9"] {
        let (status, body) = emulator.list_channels(group, "").await;
        assert_eq!(
            status, 404,
            "case {group:?}: expected 404, got {status} {body}"
        );
        assert_eq!(
            error_type(&body),
            "ResourceNotFoundException",
            "case {group:?}: expected ResourceNotFoundException, got {body}"
        );
    }
}

/// A token this service did not issue is a 400 `ValidationException`, not an empty page
/// and not a page starting somewhere arbitrary.
#[tokio::test]
async fn a_pagination_token_the_service_did_not_issue_is_refused() {
    let emulator = Emulator::new();
    assert_eq!(emulator.create_group("grp", json!({})).await.0, 200);
    assert_eq!(
        emulator.create_channel("grp", "chan", json!({})).await.0,
        200
    );

    for token in ["zzz", "not-a-token", "0"] {
        let (status, body) = emulator.list_channels("grp", &format!("?nextToken={token}")).await;
        assert_eq!(
            status, 400,
            "case {token:?}: expected 400, got {status} {body}"
        );
        assert_eq!(
            error_type(&body),
            "ValidationException",
            "case {token:?}: expected ValidationException, got {body}"
        );
    }
}

/// The fixed sentence MediaPackage answers when a request names a channel that does not
/// exist, asserted over every implemented operation that resolves a channel: the message
/// is the service's own wording, so it carries none of the request's values and does not
/// vary with the operation or with which part of the chain is missing.
#[tokio::test]
async fn a_missing_channel_is_reported_with_the_services_own_message() {
    // The sentence AWS answers with, written out here rather than read from the crate, so
    // a change to the implementation's wording fails this test instead of following it.
    let expected = "MediaPackage can't process your request because we can't find your \
                    channel. Verify your channel name or add a channel and then try again";

    let emulator = Emulator::new();
    assert_eq!(emulator.create_group("grp", json!({})).await.0, 200);
    assert_eq!(
        emulator.create_channel("grp", "present", json!({})).await.0,
        200
    );
    // A channel that existed and was deleted must read the same as one that never did.
    assert_eq!(emulator.create_channel("grp", "gone", json!({})).await.0, 200);
    assert_eq!(emulator.delete_channel("grp", "gone").await.0, 200);

    for channel in ["absent", "gone", "A_channel-9"] {
        let probes: Vec<(&str, (u16, Value))> = vec![
            ("GetChannel", emulator.get_channel("grp", channel).await),
            (
                "ListOriginEndpoints",
                emulator
                    .call(
                        "GET",
                        &format!("/channelGroup/grp/channel/{channel}/originEndpoint"),
                        None,
                    )
                    .await,
            ),
            (
                "CreateOriginEndpoint",
                emulator
                    .call(
                        "POST",
                        &format!("/channelGroup/grp/channel/{channel}/originEndpoint"),
                        Some(json!({
                            "OriginEndpointName": "ep",
                            "ContainerType": "TS",
                            // A manifest is required by the service's own input
                            // validation, so the body is otherwise valid and the refusal
                            // under test is the missing channel.
                            "HlsManifests": [{"ManifestName": "index"}],
                        })),
                    )
                    .await,
            ),
            (
                "UpdateChannel",
                emulator
                    .call(
                        "PUT",
                        &format!("/channelGroup/grp/channel/{channel}/"),
                        Some(json!({"Description": "nowhere"})),
                    )
                    .await,
            ),
            (
                "ResetChannelState",
                emulator
                    .call(
                        "POST",
                        &format!("/channelGroup/grp/channel/{channel}/reset"),
                        None,
                    )
                    .await,
            ),
            (
                "ResetOriginEndpointState",
                emulator
                    .call(
                        "POST",
                        &format!("/channelGroup/grp/channel/{channel}/originEndpoint/ep/reset"),
                        None,
                    )
                    .await,
            ),
        ];
        for (operation, (status, body)) in probes {
            let case = format!("{operation} on channel {channel:?}");
            assert_eq!(status, 404, "case {case:?}: expected 404, got {status} {body}");
            assert_eq!(
                error_type(&body),
                "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException, got {body}"
            );
            assert_eq!(
                body["Message"],
                json!(expected),
                "case {case:?}: expected the service's channel-not-found message, got {body}"
            );
            assert_eq!(
                body["ResourceTypeNotFound"],
                json!("CHANNEL"),
                "case {case:?}: expected ResourceTypeNotFound CHANNEL, got {body}"
            );
        }
    }
}

/// `MaxResults` is accepted on each published bound of the model's `ListResourceMaxResults`
/// range (1..100) and refused one past either bound.
#[tokio::test]
async fn max_results_is_accepted_on_its_bounds_and_refused_past_them() {
    let emulator = Emulator::new();
    assert_eq!(emulator.create_group("grp", json!({})).await.0, 200);
    for index in 0..3 {
        assert_eq!(
            emulator
                .create_channel("grp", &format!("chan-{index}"), json!({}))
                .await
                .0,
            200
        );
    }

    for limit in [1, 100] {
        let (status, body) = emulator.list_channels("grp", &format!("?maxResults={limit}")).await;
        assert_eq!(
            status, 200,
            "case maxResults={limit}: expected 200, got {status} {body}"
        );
    }
    for limit in [0, 101] {
        let (status, body) = emulator.list_channels("grp", &format!("?maxResults={limit}")).await;
        assert_eq!(
            status, 400,
            "case maxResults={limit}: expected 400, got {status} {body}"
        );
    }
}
