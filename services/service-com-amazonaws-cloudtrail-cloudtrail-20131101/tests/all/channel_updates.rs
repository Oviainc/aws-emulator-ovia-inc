// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateChannel`.
//!
//! Every case goes over the wire through the shared harness (`tests/all/emulator.rs`):
//! an `awsJson1_1` `POST /` with this service's published `X-Amz-Target` prefix,
//! dispatched through the same `MultiServiceRouter` the production router serves, so
//! what is asserted here is what a client sees — the HTTP status, the error code and
//! the message.
//!
//! The rules under test, each grounded in this service's Smithy model:
//!
//! * `UpdateChannel` writes ONLY the members the request carries — an absent `Name` or
//!   `Destinations` leaves the stored value alone, `Destinations` REPLACES the stored
//!   list, and `Source` is not updatable at all ("You cannot change this name after you
//!   create the channel") — and the response and the next `GetChannel` report the same
//!   record;
//! * `Channel` is the channel ARN or the bare id CloudTrail minted ("The ARN or ID (the
//!   ARN suffix) of the channel"), either form addressing the same channel; an `arn:`
//!   value that is not a channel ARN is `ChannelARNInvalidException`, and any other
//!   identifier naming no stored channel is `ChannelNotFoundException` (HTTP 400, the
//!   status the recorded service answers) naming the ARN the identifier addressed;
//! * a destination the request supplies is checked the way `CreateChannel` checks the
//!   list it was given — against the event data stores of the channel's own Region —
//!   and a refused update changes nothing;
//! * a `Name` another channel of that Region already carries is
//!   `ChannelAlreadyExistsException`, while re-setting a channel's own name is the
//!   no-op `@idempotent` promises.
//!
//! Channels are created over the wire through `CreateChannel`, and the event data
//! stores they log to are seeded into the shared registry the way
//! `channel_lifecycle.rs` seeds them (`CreateEventDataStore` is still a scaffold stub
//! with its own issue).

use serde_json::{json, Value};

use service_com_amazonaws_cloudtrail_cloudtrail_20131101::state as control_plane;
use service_com_amazonaws_cloudtrail_cloudtrail_20131101::state::channels;
use service_com_amazonaws_cloudtrail_cloudtrail_20131101::state::EventDataStoreState::{
    Enabled, PendingDeletion,
};

// The emulator-under-test harness is shared by this crate's suites; see
// `tests/all/emulator.rs`.
use super::emulator::{Emulator, Reply, HOME_REGION};

/// Another Region, for the cases about Region scoping.
const OTHER_REGION: &str = "us-west-2";

/// `Destination$Type` for a CloudTrail Lake integration channel.
const EVENT_DATA_STORE: &str = "EVENT_DATA_STORE";

/// The status every refusal in this suite renders with: `ChannelARNInvalidException`,
/// `ChannelAlreadyExistsException`, `InvalidParameterException`,
/// `EventDataStoreARNInvalidException` and `InactiveEventDataStoreException` all carry
/// `@httpError 400`, and the recorded service answers 400 for
/// `ChannelNotFoundException` and `EventDataStoreNotFoundException` too, though the
/// model annotates both `@httpError 404`.
const CLIENT_ERROR: u16 = 400;

/// An event data store id in the modelled `UUID` form (`8-4-4-4-12` lowercase hex),
/// built from `seed`. Written here rather than reused from the crate so the seeding
/// does not depend on the code under test.
fn store_id(seed: usize) -> String {
    format!("{seed:08x}-0000-4000-8000-000000000000")
}

/// The event data store ARN of `id` in `region`, in the form the model doc for
/// `AddTagsRequest$ResourceId` publishes
/// (`arn:aws:cloudtrail:us-east-2:123456789012:eventdatastore/EXAMPLE-…`).
fn store_arn(region: &str, id: &str) -> String {
    format!("arn:aws:cloudtrail:{region}:000000000000:eventdatastore/{id}")
}

/// The channel ARN of `id` in `region`, in the form the same doc publishes
/// (`arn:aws:cloudtrail:us-east-2:123456789012:channel/01234567890`).
fn expected_channel_arn(region: &str, id: &str) -> String {
    format!("arn:aws:cloudtrail:{region}:000000000000:channel/{id}")
}

/// Seed an event data store in `status` and hand back its ARN — the same map, keyed
/// the same way, that `CreateEventDataStore` will write once it is implemented.
fn seed_store(
    emulator: &Emulator,
    region: &str,
    status: control_plane::EventDataStoreState,
) -> String {
    let state = emulator.state();
    let mut store = state.lock();
    let id = store_id(store.event_data_stores.len() + 1);
    let record = control_plane::EventDataStoreRecord {
        id: id.clone(),
        name: format!("lake-{id}"),
        region: region.to_string(),
        status,
        ..Default::default()
    };
    store.event_data_stores.insert(id.clone(), record);
    store_arn(region, &id)
}

/// One `Destination` entry.
fn destination(kind: &str, location: &str) -> Value {
    json!({ "Type": kind, "Location": location })
}

/// Seed a second channel of `region` straight into the registry, and hand back its ARN.
///
/// `CreateChannel` accepts the one source a caller owns (`Custom`) and a Region logs at
/// most one channel per source, so a Region's SECOND channel is seeded the way
/// `channel_listing.rs` seeds them — a record `CreateChannel` would have written, in the
/// same map keyed the same way.
fn seed_channel(emulator: &Emulator, region: &str, name: &str, source: &str) -> String {
    let record = channels::ChannelRecord::created(region, name, source, Vec::new(), Vec::new());
    let arn = record.channel_arn.clone();
    emulator.state().lock().channels.insert(record.key(), record);
    arn
}

impl Emulator {
    /// Create a channel over the wire and hand back its ARN and its minted id.
    async fn create_channel_in(
        &self,
        region: &str,
        name: &str,
        source: &str,
        destinations: Vec<Value>,
    ) -> (String, String) {
        let reply = self
            .call_in(
                "CreateChannel",
                region,
                &json!({ "Name": name, "Source": source, "Destinations": destinations }),
            )
            .await;
        let arn = reply.expect_ok(&format!("CreateChannel {name}"))["ChannelArn"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let id = arn.rsplit('/').next().unwrap_or_default().to_string();
        (arn, id)
    }

    /// Update a channel from [`HOME_REGION`].
    async fn update_channel(&self, body: &Value) -> Reply {
        self.call("UpdateChannel", body).await
    }

    /// Read a channel back from [`HOME_REGION`].
    async fn get_channel(&self, channel: &str) -> Reply {
        self.call("GetChannel", &json!({ "Channel": channel })).await
    }
}

/// One row of the table below: the case, the request members besides `Channel` (built
/// from the three seeded store ARNs), whether the channel is addressed by ARN, and the
/// name and destination count the stored channel must carry afterwards.
type UpdateRow = (
    &'static str,
    &'static dyn Fn(&str, &str, &str) -> Value,
    bool,
    &'static str,
    usize,
);

/// RULE: `UpdateChannel` writes only the members the request carries, and the response
/// and the next read report the same record. The rows differ in which members are
/// present, in how many destinations they carry, and in which form (ARN or bare id)
/// addresses the channel; each row's expectation is the whole stored channel after the
/// call.
#[tokio::test]
async fn update_channel_writes_only_the_members_the_request_carries() {
    // (case, request members besides `Channel`, address by ARN?, expected Name,
    //  expected destination count)
    let rows: [UpdateRow; 7] = [
        (
            "neither member: an idempotent no-op",
            &|_first, _second, _third| json!({}),
            true,
            "start-name",
            1,
        ),
        (
            "the name alone, addressed by ARN",
            &|_first, _second, _third| json!({"Name": "renamed-by-arn"}),
            true,
            "renamed-by-arn",
            1,
        ),
        (
            "the name alone, addressed by the bare channel id",
            &|_first, _second, _third| json!({"Name": "renamed-by-id"}),
            false,
            "renamed-by-id",
            1,
        ),
        (
            "one destination replaces the one stored",
            &|_first, second, _third| json!({"Destinations": [destination(EVENT_DATA_STORE, second)]}),
            true,
            "start-name",
            1,
        ),
        (
            "three destinations replace the one stored",
            &|first, second, third| json!({"Destinations": [
                destination(EVENT_DATA_STORE, first),
                destination(EVENT_DATA_STORE, second),
                destination(EVENT_DATA_STORE, third),
            ]}),
            false,
            "start-name",
            3,
        ),
        (
            "both members at once",
            &|_first, _second, third| json!({
                "Name": "renamed-and-repointed",
                "Destinations": [destination(EVENT_DATA_STORE, third)],
            }),
            true,
            "renamed-and-repointed",
            1,
        ),
        (
            "its own name again: `@idempotent`, not a conflict",
            &|_first, _second, _third| json!({"Name": "start-name"}),
            true,
            "start-name",
            1,
        ),
    ];

    for (case, members, by_arn, expected_name, expected_destinations) in rows {
        // One fresh registry per row, so each row is the single update it describes.
        let emulator = Emulator::new();
        let first = seed_store(&emulator, HOME_REGION, Enabled);
        let second = seed_store(&emulator, HOME_REGION, Enabled);
        let third = seed_store(&emulator, HOME_REGION, Enabled);
        let (arn, id) = emulator
            .create_channel_in(
                HOME_REGION,
                "start-name",
                "Custom",
                vec![destination(EVENT_DATA_STORE, &first)],
            )
            .await;

        let mut request = json!({"Channel": if by_arn { &arn } else { &id }});
        let members = members(&first, &second, &third);
        for (member, value) in members.as_object().expect("an object of members") {
            request[member] = value.clone();
        }
        let updated = emulator.update_channel(&request).await;
        let updated = updated.expect_ok(case).clone();

        // The ARN and the source are the stored channel's, whatever the request
        // carried: neither is updatable.
        assert_eq!(
            (&updated["ChannelArn"], &updated["Name"], &updated["Source"]),
            (&json!(arn), &json!(expected_name), &json!("Custom")),
            "case {case}: the response does not report the updated channel, got {updated:?}"
        );
        let reported = updated["Destinations"].clone();
        assert_eq!(
            reported.as_array().map(Vec::len),
            Some(expected_destinations),
            "case {case}: expected {expected_destinations} destination(s), got {reported:?}"
        );

        // And the stored channel is exactly what the response reported — the whole
        // point of an update.
        let after = emulator.get_channel(&arn).await;
        let after = after.expect_ok(case).clone();
        assert_eq!(
            (&after["Name"], &after["Source"], &after["Destinations"]),
            (&json!(expected_name), &json!("Custom"), &reported),
            "case {case}: the stored channel is not what the response reported, got {after:?}"
        );
    }
}

/// RULE: an `arn:` identifier that is not a channel ARN is refused by its FORM; any
/// other identifier naming no stored channel is an absent channel, named by the ARN it
/// addressed. The rows differ in which part of the ARN form they break and in the form
/// the identifier is written in.
#[tokio::test]
async fn an_identifier_naming_no_channel_is_refused_and_changes_nothing() {
    let emulator = Emulator::new();
    let store = seed_store(&emulator, HOME_REGION, Enabled);
    // A real channel, so a refusal below cannot be "the registry is empty".
    let (present_arn, _) = emulator
        .create_channel_in(
            HOME_REGION,
            "present-channel",
            "Custom",
            vec![destination(EVENT_DATA_STORE, &store)],
        )
        .await;

    let absent_id = "39032a83-30fe-4581-8bd3-351632c2be72";
    // (case, `Channel`, expected error code, the subject the refusal must name)
    let rows: [(&str, String, &str, String); 9] = [
        (
            "an ARN whose resource part is not channel/<id>",
            format!("arn:aws:cloudtrail:{HOME_REGION}:000000000000:badchannel"),
            "ChannelARNInvalidException",
            format!("arn:aws:cloudtrail:{HOME_REGION}:000000000000:badchannel"),
        ),
        (
            "an ARN naming another service",
            format!("arn:aws:s3:{HOME_REGION}:000000000000:channel/{absent_id}"),
            "ChannelARNInvalidException",
            format!("arn:aws:s3:{HOME_REGION}:000000000000:channel/{absent_id}"),
        ),
        (
            "an ARN with no channel id after the prefix",
            format!("arn:aws:cloudtrail:{HOME_REGION}:000000000000:channel/"),
            "ChannelARNInvalidException",
            format!("arn:aws:cloudtrail:{HOME_REGION}:000000000000:channel/"),
        ),
        (
            "an ARN missing its Region",
            format!("arn:aws:cloudtrail::000000000000:channel/{absent_id}"),
            "ChannelARNInvalidException",
            format!("arn:aws:cloudtrail::000000000000:channel/{absent_id}"),
        ),
        (
            "an ARN with one field too few",
            format!("arn:aws:cloudtrail:{HOME_REGION}:channel/{absent_id}"),
            "ChannelARNInvalidException",
            format!("arn:aws:cloudtrail:{HOME_REGION}:channel/{absent_id}"),
        ),
        (
            // A bare identifier is read as a channel id whatever its syntax: an id
            // CloudTrail never minted names no channel, which is an absent channel.
            "a bare id that is not a minted one",
            "not-a-channel".to_string(),
            "ChannelNotFoundException",
            expected_channel_arn(HOME_REGION, "not-a-channel"),
        ),
        (
            "a well-formed ARN naming no channel",
            expected_channel_arn(HOME_REGION, absent_id),
            "ChannelNotFoundException",
            expected_channel_arn(HOME_REGION, absent_id),
        ),
        (
            // A refusal names the ARN the identifier ADDRESSED, so a bare id is
            // completed into the ARN it names in the calling Region.
            "a well-formed bare id naming no channel",
            absent_id.to_string(),
            "ChannelNotFoundException",
            expected_channel_arn(HOME_REGION, absent_id),
        ),
        (
            // The registry holds this account's channels, so another account's ARN is
            // well formed and still names nothing.
            "a channel ARN of another account",
            format!("arn:aws:cloudtrail:{HOME_REGION}:999999999999:channel/{absent_id}"),
            "ChannelNotFoundException",
            format!("arn:aws:cloudtrail:{HOME_REGION}:999999999999:channel/{absent_id}"),
        ),
    ];

    for (case, identifier, code, subject) in &rows {
        let reply = emulator
            .update_channel(&json!({"Channel": identifier, "Name": "renamed"}))
            .await;
        reply.expect_error(case, CLIENT_ERROR, code);
        let expected = match *code {
            "ChannelNotFoundException" => format!("The channel {subject} was not found."),
            _ => format!("The format of the channel ARN {subject} is not valid."),
        };
        assert_eq!(
            reply.message(),
            expected,
            "case {case}: expected the refusal {expected:?}, got {reply:?}"
        );
    }

    // The recorded instance, as one row of that rule: a BARE channel id, called in the
    // Region it was signed for
    // (`cloudtrail-tagging-channel-and-policy-not-found-error-paths#task#4`).
    let recorded_id = "a2b2b801-5178-4e9b-bc64-434d42b0200e";
    let recorded = emulator
        .call_in(
            "UpdateChannel",
            OTHER_REGION,
            &json!({"Channel": recorded_id}),
        )
        .await;
    recorded.expect_error(
        "the recorded bare channel id",
        CLIENT_ERROR,
        "ChannelNotFoundException",
    );
    assert_eq!(
        recorded.message(),
        format!(
            "The channel {} was not found.",
            expected_channel_arn(OTHER_REGION, recorded_id)
        ),
        "case the recorded bare channel id: the refusal must name the ARN the id \
         addresses, got {recorded:?}"
    );

    // Every refusal above left the one real channel exactly as it was.
    let after = emulator.get_channel(&present_arn).await;
    assert_eq!(
        after.expect_ok("the control")["Name"],
        json!("present-channel"),
        "case the control: a refused update must change nothing, got {after:?}"
    );

    // A channel lives in one Region: its ARN addresses it from anywhere, a bare id
    // only from the Region the channel is in.
    let (elsewhere_arn, elsewhere_id) = emulator
        .create_channel_in(
            OTHER_REGION,
            "regional-channel",
            "Custom",
            vec![destination(
                EVENT_DATA_STORE,
                &seed_store(&emulator, OTHER_REGION, Enabled),
            )],
        )
        .await;
    emulator
        .update_channel(&json!({"Channel": &elsewhere_id, "Name": "renamed"}))
        .await
        .expect_error(
            "a bare id of another Region's channel",
            CLIENT_ERROR,
            "ChannelNotFoundException",
        );
    let by_arn = emulator
        .update_channel(&json!({"Channel": &elsewhere_arn, "Name": "renamed-by-arn"}))
        .await;
    assert_eq!(
        by_arn.expect_ok("the ARN of another Region's channel")["Name"],
        json!("renamed-by-arn"),
        "case the ARN of another Region's channel: expected the rename to succeed, got \
         {by_arn:?}"
    );
}

/// RULE: a destination the request supplies is checked against the event data stores of
/// the channel's own Region, and a refused update changes nothing. The rows differ in
/// the destination type and in how the location fails.
#[tokio::test]
async fn a_destination_the_update_supplies_must_name_a_usable_event_data_store() {
    let emulator = Emulator::new();
    let active = seed_store(&emulator, HOME_REGION, Enabled);
    let deleting = seed_store(&emulator, HOME_REGION, PendingDeletion);
    let elsewhere = seed_store(&emulator, OTHER_REGION, Enabled);
    let (arn, _) = emulator
        .create_channel_in(
            HOME_REGION,
            "destinations-channel",
            "Custom",
            vec![destination(EVENT_DATA_STORE, &active)],
        )
        .await;

    // (case, destination, expected error code)
    let rows: [(&str, Value, &str); 6] = [
        (
            "a location that is no event data store ARN",
            destination(
                EVENT_DATA_STORE,
                &format!("arn:aws:cloudtrail:{HOME_REGION}:000000000000:trail/not-a-store"),
            ),
            "EventDataStoreARNInvalidException",
        ),
        (
            "an ARN naming another service",
            destination(EVENT_DATA_STORE, &active.replace("cloudtrail", "logs")),
            "EventDataStoreARNInvalidException",
        ),
        (
            "a well-formed ARN naming no store",
            destination(
                EVENT_DATA_STORE,
                &store_arn(HOME_REGION, "85998324-19c3-4197-86cf-3a0a481d480a"),
            ),
            "EventDataStoreNotFoundException",
        ),
        (
            // A store is reachable only from the Region it lives in, and a channel
            // logs to the lake of its own Region.
            "a store of another Region",
            destination(EVENT_DATA_STORE, &elsewhere),
            "EventDataStoreNotFoundException",
        ),
        (
            "a store waiting out its deletion window",
            destination(EVENT_DATA_STORE, &deleting),
            "InactiveEventDataStoreException",
        ),
        (
            // `DestinationType`'s other modelled value: the destination type of a
            // service-linked channel, which the service that owns it creates.
            "a service-linked destination type",
            destination("AWS_SERVICE", &active),
            "InvalidParameterException",
        ),
    ];

    for (case, bad, code) in rows {
        // Alone, and beside a destination that WOULD be accepted: one bad destination
        // refuses the whole update.
        for (how, destinations) in [
            ("alone", vec![bad.clone()]),
            (
                "beside a good one",
                vec![destination(EVENT_DATA_STORE, &active), bad.clone()],
            ),
        ] {
            let case = format!("{case}/{how}");
            let refused = emulator
                .update_channel(&json!({"Channel": &arn, "Destinations": destinations}))
                .await;
            refused.expect_error(&case, CLIENT_ERROR, code);
        }
    }

    // Nothing above changed the stored channel: it still logs to the one store it was
    // created with.
    let after = emulator.get_channel(&arn).await;
    assert_eq!(
        after.expect_ok("after the refusals")["Destinations"],
        json!([destination(EVENT_DATA_STORE, &active)]),
        "case after the refusals: a refused update must change nothing, got {after:?}"
    );
}

/// RULE: a `Name` another channel of that Region already carries is
/// `ChannelAlreadyExistsException`; a channel's own name is not, and a name taken only
/// in ANOTHER Region is not either — the check follows the channel's Region, not the
/// caller's.
#[tokio::test]
async fn a_rename_onto_a_taken_name_is_refused() {
    let emulator = Emulator::new();
    let store = seed_store(&emulator, HOME_REGION, Enabled);
    let (first_arn, _) = emulator
        .create_channel_in(
            HOME_REGION,
            "first-channel",
            "Custom",
            vec![destination(EVENT_DATA_STORE, &store)],
        )
        .await;
    let second_arn = seed_channel(
        &emulator,
        HOME_REGION,
        "second-channel",
        "aws.partner.example",
    );

    let refused = emulator
        .update_channel(&json!({"Channel": &second_arn, "Name": "first-channel"}))
        .await;
    refused.expect_error(
        "onto a name this Region has taken",
        CLIENT_ERROR,
        "ChannelAlreadyExistsException",
    );

    // Neither channel moved.
    for (case, arn, name) in [
        ("the first channel", &first_arn, "first-channel"),
        ("the second channel", &second_arn, "second-channel"),
    ] {
        let fetched = emulator.get_channel(arn).await;
        assert_eq!(
            fetched.expect_ok(case)["Name"],
            json!(name),
            "case {case}: the refused rename changed the stored channel, got {fetched:?}"
        );
    }

    // A channel's own name is not taken from it.
    let same = emulator
        .update_channel(&json!({"Channel": &first_arn, "Name": "first-channel"}))
        .await;
    assert_eq!(
        same.expect_ok("onto its own name")["Name"],
        json!("first-channel"),
        "case onto its own name: expected the idempotent no-op to succeed, got {same:?}"
    );

    // And a name only another Region has taken is free: the check follows the
    // CHANNEL's Region, even when the call is made from here by ARN.
    let (elsewhere_arn, _) = emulator
        .create_channel_in(
            OTHER_REGION,
            "elsewhere-channel",
            "Custom",
            vec![destination(
                EVENT_DATA_STORE,
                &seed_store(&emulator, OTHER_REGION, Enabled),
            )],
        )
        .await;
    let renamed = emulator
        .update_channel(&json!({"Channel": &elsewhere_arn, "Name": "first-channel"}))
        .await;
    assert_eq!(
        renamed.expect_ok("onto a name taken in another Region")["Name"],
        json!("first-channel"),
        "case onto a name taken in another Region: expected the rename to succeed, got \
         {renamed:?}"
    );
}
