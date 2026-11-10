// SPDX-License-Identifier: CC0-1.0

//! What a channel read REPORTS, end to end over HTTP.
//!
//! The harness is the one `channel_lifecycle.rs` owns, so every assertion here
//! goes through the router: a request is built, dispatched to the mounted
//! service, and the JSON response read back.
//!
//! Two rules are under test.
//!
//! The first is the one every channel-shaped response implements: a member that
//! says what the channel HAS is always reported, as a list (or a map) with
//! nothing in it when the channel has none of that thing — so `Audiences` is a
//! list for a channel created with no audience, and `LogConfiguration.LogTypes`
//! is a list for a channel nothing has configured logs on. A caller therefore
//! dereferences a collection whatever the channel was created with.
//!
//! The second is the one the create path implements for an output's ad markup:
//! an HLS output's manifests carry one form of SCTE-35 tag, so an output that
//! asked for an HLS playlist advertises an `AdMarkupType` even when its request
//! named none — the service's own form, persisted with the output, so the create
//! response and every later read report the same list. An output that asked for
//! no HLS playlist carries no HLS settings to report at all.
//!
//! The third is the round trip of the channel's log types:
//! `ConfigureLogsForChannel` says which logs the ONE channel its `ChannelName`
//! names collects, so the types `DescribeChannel` reports are the ones the most
//! recent of those calls wrote, no other channel moves, and a name the account
//! holds no channel under addresses nothing to configure.
//!
//! The tables span the classes the rules generalise over: how an output names
//! its ad-markup types (not at all, as an empty list, each modelled value on its
//! own, both together and in the other order), which playlist forms the output
//! asked for, how many audiences the channel carries, and sequences of
//! configure-logs calls that differ in the types they write, their order and how
//! often they repeat.

use serde_json::{json, Value};

use crate::channel_lifecycle::{
    table, Emulator, Reply, CHANNEL_NOT_FOUND_MESSAGE, NOT_FOUND_CODE, NOT_FOUND_STATUS,
};

/// `AdMarkupType`'s modelled values, as the generated
/// `ops::types::AD_MARKUP_TYPE_VALUES` enumerates them
/// (`smithy_query(query="shape",shape="AdMarkupType")` → `DATERANGE`,
/// `SCTE35_ENHANCED`), so the rows below span the whole enum rather than a
/// hand-written sample of it.
const AD_MARKUP_TYPES: &[&str] =
    service_com_amazonaws_mediatailor_mediatailor::ops::types::AD_MARKUP_TYPE_VALUES;

/// `LogType`'s modelled values (`ops::types::LOG_TYPE_VALUES`,
/// `smithy_query(query="shape",shape="LogType")` → `AS_RUN`): every type a
/// configure-logs request can name.
const LOG_TYPES: &[&str] =
    service_com_amazonaws_mediatailor_mediatailor::ops::types::LOG_TYPE_VALUES;

/// The ad-markup form an HLS output that named none advertises: `DATERANGE`
/// tags, the form that serves live and VOD content alike (the member's own
/// documentation ties `SCTE35_ENHANCED` to "VOD content only"), and the value
/// AWS reports for such an output.
pub(crate) const DEFAULT_AD_MARKUP_TYPE: &str = "DATERANGE";

/// The `HlsPlaylistSettings` a channel output whose request carried `requested`
/// is reported with: none when it asked for no HLS playlist, otherwise the
/// settings it sent with an ad-markup type always present — the member it sent,
/// or the service's own form when it sent none.
pub(crate) fn expected_hls_playlist_settings(requested: Option<&Value>) -> Option<Value> {
    let requested = requested?;
    let mut settings = requested.clone();
    let named = requested.get("AdMarkupType").cloned();
    settings
        .as_object_mut()
        .expect("HlsPlaylistSettings is an object")
        .insert(
            "AdMarkupType".to_string(),
            named.unwrap_or_else(|| json!([DEFAULT_AD_MARKUP_TYPE])),
        );
    Some(settings)
}

/// `ConfigureLogsForChannel`: `PUT /configureLogs/channel`, with the channel
/// name and the log types in the body
/// (`smithy_query(query="operation",operation="ConfigureLogsForChannel")`).
pub(crate) async fn configure_logs(emulator: &Emulator, name: &str, log_types: &Value) -> Reply {
    emulator
        .request(
            "PUT",
            "/configureLogs/channel",
            Some(json!({ "ChannelName": name, "LogTypes": log_types })),
        )
        .await
}

/// A channel request carrying one output, so a row below says only what that
/// output asked for.
fn channel_with(output: Value) -> Value {
    json!({ "PlaybackMode": "LOOP", "Outputs": [output] })
}

/// The outputs the ad-markup table spans: each row differs in whether it asked
/// for an HLS playlist at all, which ad-markup types it named (none, an empty
/// list, each modelled value on its own, both, and both in the other order) and
/// which other playlist settings it carried.
fn ad_markup_table() -> Vec<(&'static str, Value)> {
    let mut rows = vec![
        (
            "an HLS output naming no ad-markup type",
            json!({
                "ManifestName": "index",
                "SourceGroup": "default",
                "HlsPlaylistSettings": { "ManifestWindowSeconds": 30 },
            }),
        ),
        (
            "HLS playlist settings with nothing in them",
            json!({
                "ManifestName": "index",
                "SourceGroup": "default",
                "HlsPlaylistSettings": {},
            }),
        ),
        (
            "an HLS output naming an empty list of ad-markup types",
            json!({
                "ManifestName": "index",
                "SourceGroup": "default",
                "HlsPlaylistSettings": { "ManifestWindowSeconds": 60, "AdMarkupType": [] },
            }),
        ),        (
            "an output that asked for no HLS playlist at all",
            json!({
                "ManifestName": "index",
                "SourceGroup": "default",
                "DashPlaylistSettings": { "ManifestWindowSeconds": 30 },
            }),
        ),
        (
            "an output with no playlist settings of either form",
            json!({ "ManifestName": "index", "SourceGroup": "default" }),
        ),
        (
            "both playlist forms, the HLS one naming no ad-markup type",
            json!({
                "ManifestName": "index",
                "SourceGroup": "default",
                "HlsPlaylistSettings": { "ManifestWindowSeconds": 3600 },
                "DashPlaylistSettings": { "ManifestWindowSeconds": 30 },
            }),
        ),
        (
            "every modelled ad-markup type at once",
            json!({
                "ManifestName": "index",
                "SourceGroup": "default",
                "HlsPlaylistSettings": { "ManifestWindowSeconds": 30, "AdMarkupType": AD_MARKUP_TYPES },
            }),
        ),
        (
            "every modelled ad-markup type, in the other order",
            json!({
                "ManifestName": "index",
                "SourceGroup": "default",
                "HlsPlaylistSettings": {
                    "ManifestWindowSeconds": 30,
                    "AdMarkupType": AD_MARKUP_TYPES.iter().rev().collect::<Vec<_>>(),
                },
            }),
        ),
    ];
    // One row per modelled value, each named on its own — so the value the
    // issue did not cite is covered as well as the one it did.
    for markup in AD_MARKUP_TYPES {
        rows.push((
            markup,
            json!({
                "ManifestName": "index",
                "SourceGroup": "default",
                "HlsPlaylistSettings": { "ManifestWindowSeconds": 30, "AdMarkupType": [markup] },
            }),
        ));
    }
    rows
}

#[tokio::test]
async fn an_hls_output_reports_the_ad_markup_type_it_was_stored_with() {
    let emulator = Emulator::new();

    for (index, (case, output)) in ad_markup_table().into_iter().enumerate() {
        let name = format!("ad-markup-{index}");
        let request = channel_with(output.clone());
        let created = emulator.create(&name, &request).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected CreateChannel to answer 200, got {} {:?}",
            created.status, created.body
        );

        let expected = expected_hls_playlist_settings(output.get("HlsPlaylistSettings"));
        let reported = created.body.pointer("/Outputs/0/HlsPlaylistSettings").cloned();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected CreateChannel to report HlsPlaylistSettings \
             {expected:?}, got {reported:?}"
        );

        // The markup type was PERSISTED, not computed for the create response:
        // the read reports the same list.
        let described = emulator.describe(&name).await;
        let read_back = described.body.pointer("/Outputs/0/HlsPlaylistSettings").cloned();
        assert_eq!(
            read_back, expected,
            "case {case:?}: expected DescribeChannel to report HlsPlaylistSettings \
             {expected:?}, got {read_back:?}"
        );

        // Whatever it reports is a list of modelled values, and an output that
        // named no member advertises a type rather than nothing.
        if let Some(types) = read_back
            .as_ref()
            .and_then(|settings| settings.get("AdMarkupType"))
        {
            let types = types
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: AdMarkupType must be a list, got {types:?}"));
            assert!(
                !types.is_empty()
                    || output
                        .pointer("/HlsPlaylistSettings/AdMarkupType")
                        .is_some(),
                "case {case:?}: expected an HLS output that named no ad-markup type to \
                 advertise one, got {types:?}"
            );
            for value in types {
                assert!(
                    value
                        .as_str()
                        .is_some_and(|value| AD_MARKUP_TYPES.contains(&value)),
                    "case {case:?}: expected every reported ad-markup type to be a modelled \
                     one ({AD_MARKUP_TYPES:?}), got {value:?}"
                );
            }
        }

        // The DASH settings are reported exactly as they were sent: this rule is
        // about the HLS member alone.
        assert_eq!(
            described.body.pointer("/Outputs/0/DashPlaylistSettings"),
            output.get("DashPlaylistSettings"),
            "case {case:?}: expected DashPlaylistSettings to be reported as sent, got {:?}",
            described.body.pointer("/Outputs/0/DashPlaylistSettings")
        );
    }
}

/// The number of ad-markup types one HLS playlist may name, as
/// `state::MAX_AD_MARKUP_TYPES_PER_PLAYLIST` sets it: a CHOSEN ceiling, so one
/// stored output cannot grow without bound on a caller-sized list.
const MAX_AD_MARKUP_TYPES: usize =
    service_com_amazonaws_mediatailor_mediatailor::state::MAX_AD_MARKUP_TYPES_PER_PLAYLIST;

#[tokio::test]
async fn an_hls_playlist_is_held_to_the_number_of_ad_markup_types_one_output_carries() {
    let emulator = Emulator::new();

    // The ceiling itself, and one past it: the first is stored and reported as
    // sent, the second is refused with nothing stored.
    for (case, count) in [
        ("at the ceiling", MAX_AD_MARKUP_TYPES),
        ("one past the ceiling", MAX_AD_MARKUP_TYPES + 1),
    ] {
        let types: Vec<&str> = (0..count)
            .map(|index| AD_MARKUP_TYPES[index % AD_MARKUP_TYPES.len()])
            .collect();
        let name = format!("markup-bound-{count}");
        let created = emulator
            .create(
                &name,
                &channel_with(json!({
                    "ManifestName": "index",
                    "SourceGroup": "default",
                    "HlsPlaylistSettings": { "ManifestWindowSeconds": 30, "AdMarkupType": types },
                })),
            )
            .await;

        if count <= MAX_AD_MARKUP_TYPES {
            assert_eq!(
                created.status, 200,
                "case {case:?}: expected a playlist naming {count} ad-markup types to be \
                 stored, got {} {:?}",
                created.status, created.body
            );
            assert_eq!(
                emulator
                    .describe(&name)
                    .await
                    .get("/Outputs/0/HlsPlaylistSettings/AdMarkupType"),
                &json!(types),
                "case {case:?}: expected the types to be reported as sent"
            );
        } else {
            assert_eq!(
                created.status, 400,
                "case {case:?}: expected a playlist naming {count} ad-markup types to be \
                 refused, got {} {:?}",
                created.status, created.body
            );
            assert_eq!(
                created.error_code, "BadRequestException",
                "case {case:?}: expected BadRequestException, got {:?}",
                created.error_code
            );
            assert_eq!(
                emulator.describe(&name).await.status,
                NOT_FOUND_STATUS,
                "case {case:?}: expected the refused create to store no channel"
            );
        }
    }
}

/// The audience sets the table spans: the member absent, present but empty, one
/// audience, and several.
fn audience_table() -> Vec<(&'static str, Option<Value>)> {
    vec![
        ("no Audiences member at all", None),
        ("an empty Audiences list", Some(json!([]))),
        ("one audience", Some(json!(["en"]))),
        (
            "several audiences",
            Some(json!(["en", "fr", "de", "pt-BR"])),
        ),
    ]
}

#[tokio::test]
async fn a_channel_reports_the_audiences_it_has_as_a_list() {
    let emulator = Emulator::new();

    for (index, (case, audiences)) in audience_table().into_iter().enumerate() {
        let name = format!("audiences-{index}");
        let mut request = channel_with(json!({
            "ManifestName": "index",
            "SourceGroup": "default",
            "HlsPlaylistSettings": { "ManifestWindowSeconds": 30 },
        }));
        if let Some(audiences) = audiences.clone() {
            request
                .as_object_mut()
                .expect("the request is an object")
                .insert("Audiences".to_string(), audiences);
        }

        let created = emulator.create(&name, &request).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected CreateChannel to answer 200, got {} {:?}",
            created.status, created.body
        );

        // A channel that named no audience has none — an empty list, not an
        // absent member.
        let expected = audiences.unwrap_or_else(|| json!([]));
        assert_eq!(
            created.get("/Audiences"),
            &expected,
            "case {case:?}: expected CreateChannel to report Audiences {expected:?}, got {:?}",
            created.get("/Audiences")
        );
        let described = emulator.describe(&name).await;
        assert_eq!(
            described.get("/Audiences"),
            &expected,
            "case {case:?}: expected DescribeChannel to report Audiences {expected:?}, got {:?}",
            described.get("/Audiences")
        );
    }
}

#[tokio::test]
async fn a_channel_nothing_configured_logs_on_reports_an_empty_log_type_list() {
    let emulator = Emulator::new();

    // Every channel the lifecycle table spans: whatever a channel was created
    // with, the required log configuration is reported with a list in it.
    for (case, name, request) in table() {
        let created = emulator.create(name, &request).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected CreateChannel to answer 200, got {} {:?}",
            created.status, created.body
        );
        let described = emulator.describe(name).await;
        assert_eq!(
            described.get("/LogConfiguration/LogTypes"),
            &json!([]),
            "case {case:?}: expected a channel with no log type enabled to report an empty \
             list, got {:?}",
            described.get("/LogConfiguration")
        );
        assert!(
            described.get("/Audiences").is_array(),
            "case {case:?}: expected Audiences to be reported as a list, got {:?}",
            described.get("/Audiences")
        );
    }
}

/// The configure-logs sequences the table spans: which types each call writes,
/// in which order, and how often one request repeats. `AS_RUN` is `LogType`'s
/// only modelled value, so a row either enables it or enables nothing.
fn log_type_sequences() -> Vec<(&'static str, Vec<Value>)> {
    vec![
        ("one call enabling every modelled log type", vec![json!(LOG_TYPES)]),
        ("one call enabling nothing", vec![json!([])]),
        (
            "enabling the log types, then enabling nothing",
            vec![json!(LOG_TYPES), json!([])],
        ),
        (
            "enabling nothing, then enabling the log types",
            vec![json!([]), json!(LOG_TYPES)],
        ),
        (
            "the same request three times over",
            vec![json!(LOG_TYPES), json!(LOG_TYPES), json!(LOG_TYPES)],
        ),
        (
            "alternating, ending on the log types",
            vec![json!([]), json!(LOG_TYPES), json!([]), json!(LOG_TYPES)],
        ),
    ]
}

#[tokio::test]
async fn the_log_types_a_channel_reports_are_the_ones_the_last_call_wrote() {
    for (case, sequence) in log_type_sequences() {
        let emulator = Emulator::new();
        let name = "logged-channel";
        let created = emulator
            .create(
                name,
                &channel_with(json!({
                    "ManifestName": "index",
                    "SourceGroup": "default",
                    "HlsPlaylistSettings": { "ManifestWindowSeconds": 30 },
                })),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected the channel under test to be created, got {} {:?}",
            created.status, created.body
        );
        let before = emulator.describe(name).await.body;

        for (index, log_types) in sequence.iter().enumerate() {
            let configured = configure_logs(&emulator, name, log_types).await;
            assert_eq!(
                configured.status, 200,
                "case {case:?}: expected call {index} to answer 200, got {} {:?}",
                configured.status, configured.body
            );
            // The call answers the types AS STORED, under the channel it named.
            assert_eq!(
                configured.get("/ChannelName"),
                &json!(name),
                "case {case:?}: expected call {index} to answer the channel it named, got {:?}",
                configured.get("/ChannelName")
            );
            assert_eq!(
                configured.get("/LogTypes"),
                log_types,
                "case {case:?}: expected call {index} to answer the types it wrote \
                 {log_types:?}, got {:?}",
                configured.get("/LogTypes")
            );

            // And the read reports those types, replacing whatever was in force.
            let described = emulator.describe(name).await;
            assert_eq!(
                described.get("/LogConfiguration/LogTypes"),
                log_types,
                "case {case:?}: expected the read after call {index} to report {log_types:?}, \
                 got {:?}",
                described.get("/LogConfiguration")
            );

            // Nothing else about the channel moved.
            let strip = |mut body: Value| -> Value {
                body.as_object_mut()
                    .expect("a channel read object")
                    .remove("LogConfiguration");
                body
            };
            let (stripped_before, stripped_after) =
                (strip(before.clone()), strip(described.body.clone()));
            assert_eq!(
                stripped_after, stripped_before,
                "case {case:?}: expected call {index} to leave every other member as it stood \
                 {stripped_before:?}, got {stripped_after:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_configure_call_reaches_only_the_channel_it_names() {
    let emulator = Emulator::new();

    // Every channel the lifecycle table spans, all stored side by side.
    for (case, name, request) in table() {
        assert_eq!(
            emulator.create(name, &request).await.status,
            200,
            "case {case:?}: the channel must be created"
        );
    }

    for (case, name, _) in table() {
        let before: Vec<(String, Value)> = {
            let mut snapshots = Vec::new();
            for (_, other, _) in table() {
                snapshots.push((
                    other.to_string(),
                    emulator.describe(other).await.body.clone(),
                ));
            }
            snapshots
        };

        let configured = configure_logs(&emulator, name, &json!(LOG_TYPES)).await;
        assert_eq!(
            configured.status, 200,
            "case {case:?}: expected the configure call to be served, got {} {:?}",
            configured.status, configured.body
        );

        for (other, snapshot) in before {
            let described = emulator.describe(&other).await;
            if other == name {
                assert_eq!(
                    described.get("/LogConfiguration/LogTypes"),
                    &json!(LOG_TYPES),
                    "case {case:?}: expected {other:?} to report the types just written, got {:?}",
                    described.get("/LogConfiguration")
                );
            } else {
                assert_eq!(
                    described.body, snapshot,
                    "case {case:?}: expected configuring logs on {name:?} to leave {other:?} \
                     untouched, got {:?}",
                    described.body
                );
            }
        }
    }
}

#[tokio::test]
async fn a_configure_call_that_addresses_nothing_is_refused_and_writes_nothing() {
    let emulator = Emulator::new();

    // Stored, so the refusals below are about the request and not about an
    // empty store.
    let stored = "stored";
    assert_eq!(
        emulator
            .create(
                stored,
                &channel_with(json!({ "ManifestName": "index", "SourceGroup": "default" }))
            )
            .await
            .status,
        200,
        "the channel the refusals are compared against must be created"
    );
    assert_eq!(
        configure_logs(&emulator, stored, &json!(LOG_TYPES)).await.status,
        200,
        "the channel under test must accept log types while it is stored"
    );

    // A name the account holds no channel under addresses nothing to configure.
    for case in ["absent", "STORED", "stored-2", "stor"] {
        let refused = configure_logs(&emulator, case, &json!(LOG_TYPES)).await;
        assert_eq!(
            refused.status, NOT_FOUND_STATUS,
            "case {case:?}: expected a channel the account does not hold to be refused as not \
             found, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code, NOT_FOUND_CODE,
            "case {case:?}: expected {NOT_FOUND_CODE}, got {:?}",
            refused.error_code
        );
        assert_eq!(
            refused.message(),
            CHANNEL_NOT_FOUND_MESSAGE,
            "case {case:?}: expected the channel-miss sentence, got {:?}",
            refused.message()
        );

        // Nothing was stored: the name is still free and the channel that does
        // exist keeps the types it was given.
        assert_eq!(
            emulator.describe(case).await.status,
            NOT_FOUND_STATUS,
            "case {case:?}: expected the refused call to store no channel"
        );
    }

    // A log type the model does not declare is not a log type: the request is
    // refused before anything is written.
    let refused = configure_logs(&emulator, stored, &json!(["NOT_A_LOG_TYPE"])).await;
    assert_eq!(
        refused.status, 400,
        "expected an unmodelled log type to be refused with 400, got {} {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        emulator
            .describe(stored)
            .await
            .get("/LogConfiguration/LogTypes"),
        &json!(LOG_TYPES),
        "expected every refused call to leave the stored log types in force"
    );
}
