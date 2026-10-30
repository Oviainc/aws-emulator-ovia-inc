// SPDX-License-Identifier: CC0-1.0

//! The engine version PIN a channel carries, as both writers of it set it, end to
//! end over the restJson1 wire.
//!
//! The rule under test: `ChannelEngineVersionRequest$Version` is "The build
//! identifier of the engine version to use for this channel. Specify 'DEFAULT' to
//! reset to the default version", so for EVERY operation that writes a channel's
//! pin from that shape — `CreateChannel` and `UpdateChannel` — the documented
//! reset value un-pins the channel (the state a channel that named no engine
//! version is in, and the one the default version is served from) and any other
//! value is stored as the build identifier the caller pinned.
//!
//! The rows differ along what the rule generalises over: which operation writes
//! the pin, the value it names (the reset literal, a build identifier of this
//! emulator's labelling form, a build identifier of another form), the pin the
//! channel already carried, and the order the two writers are applied in. Each
//! row is read back through every channel-shaped reader the pin reaches: the
//! writer's own reply, `DescribeChannel`'s full description and `ListChannels`'
//! summary.

use serde_json::{json, Value};
use service_com_amazonaws_medialive_medialive::state::channel_engine_version_label;
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::wait_for_the_clock;
use crate::support::member;
use crate::wire::{call, medialive};

/// The one `Version` value that is a RESET rather than a build identifier, spelled
/// as `ChannelEngineVersionRequest$Version` documents it: "Specify 'DEFAULT' to
/// reset to the default version".
const RESET: &str = "DEFAULT";

/// One row: the value the create names, the pin every read must then report, and
/// the same pair for an update applied afterwards.
struct Row<'a> {
    case: &'static str,
    /// The `channelEngineVersion.version` the create names, or `None` when the
    /// create names no engine version member at all.
    created: Option<&'a str>,
    /// The build identifier every read of the channel must report after the
    /// create, or `None` for an un-pinned channel.
    pinned_after_create: Option<&'a str>,
    /// The `channelEngineVersion.version` a following update names, or `None` when
    /// the row applies no update.
    updated: Option<&'a str>,
    /// The build identifier every read must report after that update.
    pinned_after_update: Option<&'a str>,
}

/// `CreateChannel`, answering the channel it reports.
async fn create_channel(router: &MultiServiceRouter, case: &str, body: Value) -> Value {
    let body = crate::support::resolved_channel_body(router, body).await;
    let (status, response) = call(router, "POST", "/prod/channels", Some(body)).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected CreateChannel to answer 201, got {status}: {response}"
    );
    response["channel"].clone()
}

/// `UpdateChannel`, answering the channel it reports.
async fn update_channel(router: &MultiServiceRouter, case: &str, id: &str, body: Value) -> Value {
    let (status, response) = call(
        router,
        "PUT",
        &format!("/prod/channels/{id}"),
        Some(body),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected UpdateChannel to answer 200, got {status}: {response}"
    );
    response["channel"].clone()
}

/// `DescribeChannel`.
async fn describe_channel(router: &MultiServiceRouter, case: &str, id: &str) -> Value {
    let (status, body) = call(router, "GET", &format!("/prod/channels/{id}"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DescribeChannel to answer 200, got {status}: {body}"
    );
    body
}

/// The `ListChannels` summary of the channel `id`.
async fn listed_channel(router: &MultiServiceRouter, case: &str, id: &str) -> Value {
    let (status, body) = call(router, "GET", "/prod/channels", None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListChannels to answer 200, got {status}: {body}"
    );
    body["channels"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .find(|channel| channel["id"] == json!(id))
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: ListChannels reported no channel {id}: {body}"))
}

/// The `channelEngineVersion` a channel pinned to `expected` must report: the
/// build identifier it was pinned with, or no member at all when it is un-pinned.
///
/// The account's engine version catalogue is empty here — the service publishes
/// engine versions and no operation of this one creates them — so a reported pin
/// carries its identifier and no `expirationDate`, which the expiry suite covers.
fn expected_engine_version(expected: Option<&str>) -> Value {
    expected.map_or(Value::Null, |version| json!({ "version": version }))
}

/// Assert one channel-shaped reply reports the pin `expected`.
fn assert_pin(case: &str, reader: &str, expected: Option<&str>, channel: &Value) {
    let want = expected_engine_version(expected);
    let reported = member(channel, "channelEngineVersion");
    assert_eq!(
        reported, want,
        "case {case:?}/{reader}: expected channelEngineVersion {want:?}, got {reported:?}"
    );
}

/// Assert every channel-shaped reader of the channel `id` reports the pin
/// `expected`, the writer's own reply included.
async fn assert_pin_everywhere(
    router: &MultiServiceRouter,
    case: &str,
    stage: &str,
    id: &str,
    written: &Value,
    expected: Option<&str>,
) {
    assert_pin(case, &format!("{stage} reply"), expected, written);
    let described = describe_channel(router, case, id).await;
    assert_pin(case, &format!("{stage}/DescribeChannel"), expected, &described);
    let listed = listed_channel(router, case, id).await;
    assert_pin(case, &format!("{stage}/ListChannels"), expected, &listed);
}

/// Rule: the documented reset value un-pins a channel and every other value pins
/// that build identifier, whichever operation writes the pin.
#[tokio::test]
async fn the_engine_version_reset_value_un_pins_a_channel_on_either_writer() {
    // Two build identifiers of the labelling form this crate's engine version
    // catalogue names its releases by (`state::channel_engine_version_label`),
    // and one of another form entirely: `Version` is documented only as "The
    // build identifier of the engine version to use for this channel", so a
    // value's FORM is nothing this rule turns on — only the reset value is
    // special.
    let first = channel_engine_version_label(2);
    let second = channel_engine_version_label(3);
    let foreign = "engine-2026.10-beta";

    let table = [
        Row {
            case: "no engine version named at all",
            created: None,
            pinned_after_create: None,
            updated: None,
            pinned_after_update: None,
        },
        Row {
            case: "the reset value on the create",
            created: Some(RESET),
            pinned_after_create: None,
            updated: None,
            pinned_after_update: None,
        },
        Row {
            case: "a build identifier on the create",
            created: Some(&first),
            pinned_after_create: Some(&first),
            updated: None,
            pinned_after_update: Some(&first),
        },
        Row {
            case: "a build identifier of another form on the create",
            created: Some(foreign),
            pinned_after_create: Some(foreign),
            updated: None,
            pinned_after_update: Some(foreign),
        },
        Row {
            case: "the reset value on the create, then a pin on the update",
            created: Some(RESET),
            pinned_after_create: None,
            updated: Some(&second),
            pinned_after_update: Some(&second),
        },
        Row {
            case: "a pin on the create, then the reset value on the update",
            created: Some(&first),
            pinned_after_create: Some(&first),
            updated: Some(RESET),
            pinned_after_update: None,
        },
        Row {
            case: "a pin on the create, then another pin on the update",
            created: Some(&first),
            pinned_after_create: Some(&first),
            updated: Some(&second),
            pinned_after_update: Some(&second),
        },
        Row {
            case: "no engine version on the create, then the reset value on the update",
            created: None,
            pinned_after_create: None,
            updated: Some(RESET),
            pinned_after_update: None,
        },
        Row {
            case: "the reset value on the create and on the update",
            created: Some(RESET),
            pinned_after_create: None,
            updated: Some(RESET),
            pinned_after_update: None,
        },
    ];

    for row in table {
        let case = row.case;
        let router = medialive();

        let mut request = json!({ "name": "engine-version-pin" });
        if let Some(version) = row.created {
            request["channelEngineVersion"] = json!({ "version": version });
        }
        let created = create_channel(&router, case, request).await;
        let id = created["id"].as_str().unwrap_or_default().to_string();
        assert_pin_everywhere(
            &router,
            case,
            "create",
            &id,
            &created,
            row.pinned_after_create,
        )
        .await;

        // An update is accepted once the create's transient state has settled.
        wait_for_the_clock();

        if let Some(version) = row.updated {
            let updated = update_channel(
                &router,
                case,
                &id,
                json!({ "channelEngineVersion": { "version": version } }),
            )
            .await;
            assert_pin_everywhere(
                &router,
                case,
                "update",
                &id,
                &updated,
                row.pinned_after_update,
            )
            .await;
        }
    }
}

/// Rule, stated as the invariant between the two writers: a channel un-pinned by
/// the reset value is in the SAME state whichever operation reset it, and that is
/// the state of a channel that never named an engine version — so the three
/// channels are indistinguishable by their engine version, and each can be pinned
/// again afterwards.
#[tokio::test]
async fn the_two_writers_of_the_pin_agree_about_the_reset_value() {
    let case = "reset through each writer";
    let router = medialive();
    let pin = channel_engine_version_label(1);

    // Un-pinned because it named no engine version.
    let never = create_channel(&router, case, json!({ "name": "never-pinned" })).await;
    let never_id = never["id"].as_str().unwrap_or_default().to_string();

    // Un-pinned by the create, which named the reset value.
    let reset_on_create = create_channel(
        &router,
        case,
        json!({
            "name": "reset-on-create",
            "channelEngineVersion": { "version": RESET },
        }),
    )
    .await;
    let reset_on_create_id = reset_on_create["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // Un-pinned by an update, which named the reset value on a pinned channel.
    let pinned = create_channel(
        &router,
        case,
        json!({
            "name": "reset-on-update",
            "channelEngineVersion": { "version": pin },
        }),
    )
    .await;
    let reset_on_update_id = pinned["id"].as_str().unwrap_or_default().to_string();
    wait_for_the_clock();
    update_channel(
        &router,
        case,
        &reset_on_update_id,
        json!({ "channelEngineVersion": { "version": RESET } }),
    )
    .await;
    wait_for_the_clock();

    let reference = member(&describe_channel(&router, case, &never_id).await, "channelEngineVersion");
    for (writer, id) in [
        ("CreateChannel", &reset_on_create_id),
        ("UpdateChannel", &reset_on_update_id),
    ] {
        let reported = member(
            &describe_channel(&router, case, id).await,
            "channelEngineVersion",
        );
        assert_eq!(
            reported, reference,
            "case {case:?}/{writer}: expected a channel reset through {writer} to report the \
             engine version a channel that never named one reports ({reference:?}), got \
             {reported:?}"
        );

        // The reset left a channel that can be pinned again, so what it cleared
        // was the pin and not the channel's ability to carry one.
        let repinned = update_channel(
            &router,
            case,
            id,
            json!({ "channelEngineVersion": { "version": pin } }),
        )
        .await;
        assert_pin(case, writer, Some(&pin), &repinned);
        wait_for_the_clock();
    }
}
