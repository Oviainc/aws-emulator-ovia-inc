// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for CloudTrail Lake channels (`CreateChannel` / `GetChannel`).
//!
//! Every case goes over the wire through the shared harness
//! (`tests/all/emulator.rs`): an `awsJson1_1` `POST /` with this service's
//! published `X-Amz-Target` prefix, dispatched through the same
//! `MultiServiceRouter` the production router serves, so what is asserted here is
//! what a client sees — including the HTTP status and the error code.
//!
//! The rules under test, each grounded in this service's Smithy model:
//!
//! * `CreateChannel` persists a channel ONLY when every `Destinations[].Location`
//!   is the ARN of an event data store that exists in the channel's own Region and
//!   is usable, and it mints
//!   `arn:aws:cloudtrail:<region>:<account>:channel/<uuid>` for it;
//!   each way a destination can fail is refused with its own modelled error
//!   (`EventDataStoreARNInvalidException` 400, `EventDataStoreNotFoundException`
//!   400 — the status the recorded service answers, not the shape's `@httpError`
//!   404 — `InactiveEventDataStoreException` 400, `InvalidParameterException` 400)
//!   and persists nothing;
//! * a channel name is taken within the Region it was created in, and a Region
//!   logs at most one channel per source ("A maximum of one channel is allowed per
//!   source") — both `ChannelAlreadyExistsException`, and both per Region, so the
//!   same name and source are free in another Region;
//! * `Source` is `Custom` — the one source a channel can be created for here, since
//!   a partner source names a partner integration this emulator does not carry — and
//!   every other value is refused with `InvalidSourceException`;
//! * `Tags` obeys the documented 50-tags-per-resource limit and carries no
//!   duplicate key, and the registry's own bound refuses the channel past it
//!   (`ChannelMaxLimitExceededException`);
//! * a channel the registry holds is a tagged resource like the other three, so
//!   the tags `CreateChannel` persisted read back through `ListTags` and are
//!   removable through `RemoveTags`;
//! * `GetChannel` reads a channel back by its ARN or by the bare `UUID` CloudTrail
//!   minted for it, reports the stored values, refuses an `arn:` value that is not a
//!   channel ARN with `ChannelARNInvalidException` and an identifier that names no
//!   stored channel — another account's, another Region's by bare id, or an id
//!   CloudTrail never minted — with `ChannelNotFoundException` (HTTP 400, the status
//!   the recorded service answers; the identifier reading and both refusals are the
//!   registry's shared ones, which `DeleteChannel` and `ListChannels` use too).
//!
//! How a request's `X-Amz-Target` prefix is read is pinned by
//! `tests/all/target_prefix_dispatch.rs`, so these cases use the shared harness's
//! prefix and do not re-test it.

use serde_json::{json, Value};

use service_com_amazonaws_cloudtrail_cloudtrail_20131101::state as control_plane;
use service_com_amazonaws_cloudtrail_cloudtrail_20131101::state::channels;
use service_com_amazonaws_cloudtrail_cloudtrail_20131101::state::EventDataStoreState::{
    Created, Enabled, PendingDeletion,
};

// The emulator-under-test harness is shared by this crate's suites; see
// `tests/all/emulator.rs`.


use super::emulator::{Emulator, HOME_REGION};

/// Another Region, for the cases about Region scoping.
const OTHER_REGION: &str = "us-west-2";

/// `Destination$Type` for a CloudTrail Lake integration channel.
const EVENT_DATA_STORE: &str = "EVENT_DATA_STORE";

/// A `UUID` that no seeded event data store carries, for the not-found cases.
const UNKNOWN_UUID: &str = "85998324-19c3-4197-86cf-3a0a481d480a";

/// An event data store id in the modelled `UUID` form (`8-4-4-4-12` lowercase hex),
/// built from `seed` so each seeded store carries its own id without repeating the
/// shape. Written here rather than reused from the crate so the seeding does not
/// depend on the code under test.
fn store_id(seed: usize) -> String {
    format!("{seed:08x}-0000-4000-8000-000000000000")
}

/// Seed an event data store in `status` and hand back its ARN.
///
/// `CreateEventDataStore` is still a scaffold stub (it has its own issue), so a
/// destination a channel can log to is seeded through the shared registry the
/// handler reads — the same map keyed the same way that operation will write.
fn seed_store(
    emulator: &Emulator,
    region: &str,
    name: &str,
    status: control_plane::EventDataStoreState,
) -> String {
    let state = emulator.state();
    let mut store = state.lock();
    let id = store_id(store.event_data_stores.len() + 1);
    let record = control_plane::EventDataStoreRecord {
        id: id.clone(),
        name: name.to_string(),
        region: region.to_string(),
        status,
        ..Default::default()
    };
    store.event_data_stores.insert(id.clone(), record);
    control_plane::event_data_store_arn(region, &id)
}

/// Whether the account holds a channel called `name` in `region`.
fn has_channel_named(emulator: &Emulator, region: &str, name: &str) -> bool {
    emulator.state().lock().channel_named(region, name).is_some()
}

/// How many channels `region` holds.
fn channels_in(emulator: &Emulator, region: &str) -> usize {
    emulator.state().lock().channels_in(region)
}

/// Seed `count` channels in `region`, to drive the registry's own bound.
///
/// Each one is a record `CreateChannel` would have written, with its own minted
/// id and a distinct name and source, inserted straight into the registry the
/// handler reads.
fn seed_channels(emulator: &Emulator, region: &str, count: usize) {
    let state = emulator.state();
    let mut store = state.lock();
    for index in 0..count {
        let record = channels::ChannelRecord::created(
            region,
            &format!("seeded-{index}"),
            &format!("seeded-source-{index}"),
            Vec::new(),
            Vec::new(),
        );
        store.channels.insert(record.key(), record);
    }
}

/// One `Destination` entry.
fn destination(kind: &str, location: &str) -> Value {
    json!({ "Type": kind, "Location": location })
}

/// A `CreateChannel` request body.
fn create_channel(name: &str, source: &str, destinations: Vec<Value>) -> Value {
    json!({ "Name": name, "Source": source, "Destinations": destinations })
}

/// True when `value` is written the way the model's `UUID` shape is
/// (`@length 36`, `@pattern ^[a-f0-9\-]+$`): lowercase-hex groups of 8-4-4-4-12.
/// Written here rather than reused from the crate so the assertion does not depend
/// on the code it checks.
fn is_uuid_form(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == 5
        && groups
            .iter()
            .zip([8, 4, 4, 4, 12])
            .all(|(group, length)| {
                group.len() == length
                    && group
                        .bytes()
                        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            })
}

/// The string at `member`, or `""`.
fn string_member(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

#[tokio::test]
async fn create_channel_mints_an_arn_and_get_channel_reads_the_channel_back() {
    let emulator = Emulator::new();

    // The rule generalises over how many destinations a channel logs to and over
    // which stores they are, so the rows differ along both — one store, two, a store
    // in another of its statuses, and the same store named twice. Each row runs in
    // its own Region because `Custom` is the one source this emulator has and a
    // Region holds at most one channel per source (see
    // `create_channel_allows_one_channel_per_source_in_a_region`).
    let cases: Vec<(&str, &str, &[usize])> = vec![
        ("one-destination", HOME_REGION, &[0]),
        ("two-destinations", OTHER_REGION, &[0, 1]),
        ("destination-in-the-created-state", "eu-west-1", &[1]),
        ("the-same-destination-twice", "ap-southeast-2", &[0, 0]),
    ];

    for (case, region, picks) in cases {
        // Two stores per Region: one ingesting, one still being created. Both are
        // usable destinations; only a store waiting out its deletion window is not.
        let stores = [
            seed_store(&emulator, region, "eds-one", Enabled),
            seed_store(&emulator, region, "eds-two", Created),
        ];
        let destinations: Vec<Value> = picks
            .iter()
            .map(|pick| destination(EVENT_DATA_STORE, &stores[*pick]))
            .collect();

        let source = "Custom";
        let created = emulator
            .call_in(
                "CreateChannel",
                region,
                &create_channel(case, source, destinations.clone()),
            )
            .await;
        let body = created.expect_ok(case);

        let arn = string_member(body, "ChannelArn");
        let prefix = format!(
            "arn:aws:cloudtrail:{region}:{}:channel/",
            engine::EMULATOR_ACCOUNT_ID
        );
        let channel_id = arn.strip_prefix(&prefix).unwrap_or_default().to_string();
        assert!(
            is_uuid_form(&channel_id),
            "case {case}: expected a channel ARN {prefix}<uuid>, got {arn:?}"
        );
        assert_eq!(
            string_member(body, "Name"),
            case,
            "case {case}: expected the requested Name, got {body:?}"
        );
        assert_eq!(
            string_member(body, "Source"),
            source,
            "case {case}: expected the requested Source, got {body:?}"
        );
        assert_eq!(
            body.get("Destinations"),
            Some(&Value::Array(destinations.clone())),
            "case {case}: expected the requested Destinations, got {body:?}"
        );
        assert!(
            body.get("Tags").is_none(),
            "case {case}: a channel created without tags must not report Tags, got {body:?}"
        );

        // Read it back, by ARN and by the bare UUID the model also accepts; both
        // report the stored values, and reading twice reports the same thing.
        for (how, identifier) in [("arn", arn.clone()), ("uuid", channel_id.clone())] {
            for read in 1..=2 {
                let fetched = emulator
                    .call_in("GetChannel", region, &json!({ "Channel": identifier }))
                    .await;
                let fetched = fetched.expect_ok(case);
                assert_eq!(
                    (
                        string_member(fetched, "ChannelArn"),
                        string_member(fetched, "Name"),
                        string_member(fetched, "Source"),
                        fetched.get("Destinations").cloned(),
                    ),
                    (
                        arn.clone(),
                        case.to_string(),
                        source.to_string(),
                        Some(Value::Array(destinations.clone())),
                    ),
                    "case {case}: read {read} by {how} must report the stored channel, \
                     got {fetched:?}"
                );
                assert_eq!(
                    fetched.get("SourceConfig"),
                    Some(&json!({ "ApplyToAllRegions": false, "AdvancedEventSelectors": [] })),
                    "case {case}: read {read} by {how} must report the stored SourceConfig, \
                     got {fetched:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn create_channel_refuses_every_destination_that_is_not_a_usable_event_data_store() {
    let emulator = Emulator::new();
    let active = seed_store(&emulator, HOME_REGION, "eds-active", Enabled);
    let deleting =
        seed_store(&emulator, HOME_REGION, "eds-deleting", PendingDeletion);
    let elsewhere = seed_store(&emulator, OTHER_REGION, "eds-elsewhere", Enabled);
    let account = engine::EMULATOR_ACCOUNT_ID;

    // Each row is one way a destination can fail, with the modelled error and
    // status that failure answers with. Every Location here satisfies the member's
    // modelled `@pattern ^[a-zA-Z0-9._/\-:*]+$`, so the boundary validator passes
    // it and the handler is what refuses it.
    let cases: Vec<(&str, Value, u16, &str)> = vec![
        (
            // The recorded instance: a CloudTrail ARN whose resource type is not
            // `eventdatastore`.
            "resource-type-misspelled",
            destination(
                EVENT_DATA_STORE,
                &format!("arn:aws:cloudtrail:{OTHER_REGION}:{account}:eventdatastorewrong/{UNKNOWN_UUID}"),
            ),
            400,
            "EventDataStoreARNInvalidException",
        ),
        (
            "another-cloudtrail-resource-type",
            destination(
                EVENT_DATA_STORE,
                &format!("arn:aws:cloudtrail:{HOME_REGION}:{account}:trail/my-trail"),
            ),
            400,
            "EventDataStoreARNInvalidException",
        ),
        (
            "another-service",
            destination(
                EVENT_DATA_STORE,
                &format!("arn:aws:logs:{HOME_REGION}:{account}:eventdatastore/{UNKNOWN_UUID}"),
            ),
            400,
            "EventDataStoreARNInvalidException",
        ),
        (
            "resource-id-not-a-uuid",
            destination(
                EVENT_DATA_STORE,
                &format!("arn:aws:cloudtrail:{HOME_REGION}:{account}:eventdatastore/not-a-uuid"),
            ),
            400,
            "EventDataStoreARNInvalidException",
        ),
        (
            "account-id-not-twelve-digits",
            destination(
                EVENT_DATA_STORE,
                &format!("arn:aws:cloudtrail:{HOME_REGION}:12345:eventdatastore/{UNKNOWN_UUID}"),
            ),
            400,
            "EventDataStoreARNInvalidException",
        ),
        (
            "missing-arn-fields",
            destination(EVENT_DATA_STORE, "arn:aws:cloudtrail:eventdatastore/x"),
            400,
            "EventDataStoreARNInvalidException",
        ),
        (
            "bare-uuid-is-not-an-arn",
            destination(EVENT_DATA_STORE, UNKNOWN_UUID),
            400,
            "EventDataStoreARNInvalidException",
        ),
        (
            "well-formed-but-no-such-store",
            destination(
                EVENT_DATA_STORE,
                &format!("arn:aws:cloudtrail:{HOME_REGION}:{account}:eventdatastore/{UNKNOWN_UUID}"),
            ),
            // The recorded service answers 400 for this one, not the shape's
            // modelled 404.
            400,
            "EventDataStoreNotFoundException",
        ),
        (
            // Known id, but the ARN names a Region the store is not in: a
            // different ARN, so a different store, and this account holds none.
            "right-id-wrong-region",
            destination(
                EVENT_DATA_STORE,
                &elsewhere.replace(OTHER_REGION, HOME_REGION),
            ),
            400,
            "EventDataStoreNotFoundException",
        ),
        (
            // The store EXISTS, under its own ARN — but in another Region, and a
            // channel logs only to a store it shares a Region with.
            "store-in-another-region",
            destination(EVENT_DATA_STORE, &elsewhere),
            400,
            "EventDataStoreNotFoundException",
        ),
        (
            "store-waiting-out-its-deletion-window",
            destination(EVENT_DATA_STORE, &deleting),
            400,
            "InactiveEventDataStoreException",
        ),
        (
            "service-linked-destination-type",
            destination("AWS_SERVICE", &active),
            400,
            "InvalidParameterException",
        ),
    ];

    for (case, bad, status, code) in cases {
        let name = format!("ch-{case}");
        let location = string_member(&bad, "Location");
        // A destination refused for its Location names that Location; one refused
        // for its Type names the type instead, since the Location was never read.
        let named = match code {
            "InvalidParameterException" => string_member(&bad, "Type"),
            _ => location,
        };

        // Alone, and beside a destination that WOULD be accepted: one bad
        // destination refuses the whole channel.
        for (how, destinations) in [
            ("alone", vec![bad.clone()]),
            (
                "beside-a-good-one",
                vec![destination(EVENT_DATA_STORE, &active), bad.clone()],
            ),
        ] {
            let case = format!("{case}/{how}");
            let refused = emulator
                .call("CreateChannel", &create_channel(&name, "Custom", destinations))
                .await;
            refused.expect_error(&case, status, code);
            assert!(
                refused.message().contains(&named),
                "case {case}: the refusal must name {named}, got {:?}",
                refused.message()
            );
            assert!(
                !has_channel_named(&emulator, HOME_REGION, &name),
                "case {case}: a refused CreateChannel must persist no channel"
            );
        }
    }

    // And the control: a channel on that name and source is still creatable
    // afterwards, because nothing was persisted.
    let created = emulator
        .call(
            "CreateChannel",
            &create_channel(
                "ch-resource-type-misspelled",
                "Custom",
                vec![destination(EVENT_DATA_STORE, &active)],
            ),
        )
        .await;
    created.expect_ok("after-refusals");
}

#[tokio::test]
async fn create_channel_allows_one_channel_per_source_in_a_region() {
    let emulator = Emulator::new();
    let home = seed_store(&emulator, HOME_REGION, "eds-home", Enabled);
    let other = seed_store(&emulator, OTHER_REGION, "eds-other", Enabled);

    let first = emulator
        .call(
            "CreateChannel",
            &create_channel("shared-name", "Custom", vec![destination(EVENT_DATA_STORE, &home)]),
        )
        .await;
    let first_arn = string_member(first.expect_ok("first"), "ChannelArn");

    // The same name in the same Region is a conflict, never an overwrite — and so is
    // a DIFFERENT name on a source the Region already logs: "A maximum of one
    // channel is allowed per source" (`CreateChannelRequest$Source`).
    for (case, name) in [("duplicate-name", "shared-name"), ("same-source", "other-name")] {
        let conflict = emulator
            .call(
                "CreateChannel",
                &create_channel(name, "Custom", vec![destination(EVENT_DATA_STORE, &home)]),
            )
            .await;
        conflict.expect_error(case, 400, "ChannelAlreadyExistsException");
        assert!(
            !has_channel_named(&emulator, HOME_REGION, "other-name"),
            "case {case}: a refused CreateChannel must persist no channel"
        );
        assert_eq!(
            channels_in(&emulator, HOME_REGION),
            1,
            "case {case}: a refused CreateChannel must leave {HOME_REGION} holding only \
             the one channel it already had"
        );
    }

    // Another Region is another channel: the same name AND the same source are free
    // there, because the bound is per Region.
    let second = emulator
        .call_in(
            "CreateChannel",
            OTHER_REGION,
            &create_channel(
                "shared-name",
                "Custom",
                vec![destination(EVENT_DATA_STORE, &other)],
            ),
        )
        .await;
    let second_arn = string_member(second.expect_ok("other-region"), "ChannelArn");
    assert_ne!(
        first_arn, second_arn,
        "case other-region: two Regions' channels of one name must be two channels, \
         got {first_arn} twice"
    );
    assert!(
        second_arn.contains(&format!(":{OTHER_REGION}:")),
        "case other-region: expected a channel ARN in {OTHER_REGION}, got {second_arn}"
    );

    // A channel identifier addresses the channel in the Region the identifier pins:
    // a channel ARN carries its own Region, so it reads the channel it names from
    // wherever the call is made, while a BARE id carries none and so names a channel
    // of the calling Region only — the shared reading in `state::channels`, which
    // `DeleteChannel` and `ListChannels` resolve an identifier by too.
    let second_id = second_arn
        .rsplit('/')
        .next()
        .expect("a channel ARN ends in its id")
        .to_string();
    let fetched = emulator
        .call_in(
            "GetChannel",
            HOME_REGION,
            &json!({ "Channel": second_arn.clone() }),
        )
        .await;
    assert_eq!(
        string_member(fetched.expect_ok("cross-region-arn"), "ChannelArn"),
        second_arn,
        "case cross-region-arn: an ARN pins its own Region, so it must read the \
         channel it names, got {fetched:?}"
    );
    let fetched = emulator
        .call_in("GetChannel", HOME_REGION, &json!({ "Channel": second_id }))
        .await;
    fetched.expect_error("cross-region-uuid", 400, "ChannelNotFoundException");
    // …and the Region it was created in reads it back by its ARN as well.
    let fetched = emulator
        .call_in(
            "GetChannel",
            OTHER_REGION,
            &json!({ "Channel": second_arn.clone() }),
        )
        .await;
    assert_eq!(
        string_member(fetched.expect_ok("own-region"), "ChannelArn"),
        second_arn,
        "case own-region: expected the channel created in {OTHER_REGION}, got {fetched:?}"
    );
}

#[tokio::test]
async fn create_channel_refuses_a_channel_past_the_registry_bound() {
    let emulator = Emulator::new();
    let store = seed_store(&emulator, HOME_REGION, "eds-quota", Enabled);
    let destinations = vec![destination(EVENT_DATA_STORE, &store)];

    // The bound is on the WHOLE registry (see `state::CHANNEL_QUOTA`), so filling
    // it from one Region is what refuses the next channel anywhere. The records are
    // seeded straight into the registry, each one what `CreateChannel` would have
    // written.
    seed_channels(&emulator, OTHER_REGION, channels::CHANNEL_QUOTA);
    assert_eq!(
        channels_in(&emulator, OTHER_REGION),
        channels::CHANNEL_QUOTA,
        "case at-the-bound: expected the registry seeded to its bound"
    );

    let refused = emulator
        .call(
            "CreateChannel",
            &create_channel("past-the-bound", "Custom", destinations.clone()),
        )
        .await;
    refused.expect_error("past-the-bound", 400, "ChannelMaxLimitExceededException");
    assert!(
        !has_channel_named(&emulator, HOME_REGION, "past-the-bound"),
        "case past-the-bound: a refused CreateChannel must persist no channel"
    );

    // One release and the next channel is accepted again, so the bound is the
    // registry's size and not a latch.
    let released = emulator.state().lock().channels.keys().next().cloned();
    let released = released.expect("the registry holds the seeded channels");
    emulator.state().lock().channels.remove(&released);
    let created = emulator
        .call(
            "CreateChannel",
            &create_channel("past-the-bound", "Custom", destinations),
        )
        .await;
    created.expect_ok("after-a-release");
}

#[tokio::test]
async fn create_channel_checks_source_and_tags() {
    let emulator = Emulator::new();
    let store = seed_store(&emulator, HOME_REGION, "eds-tags", Enabled);
    let destinations = vec![destination(EVENT_DATA_STORE, &store)];

    // `Source` is modelled `@pattern .*`, so every row here reaches the handler.
    // `Custom` is the one source a channel can be created for: a partner source
    // names a partner integration, and this emulator carries none. Only the
    // refusals run in this Region — the accepting row is the one in
    // `create_channel_allows_one_channel_per_source_in_a_region`, since a Region
    // holds at most one channel per source.
    let sources: Vec<(&str, &str)> = vec![
        ("wrong-case", "custom"),
        ("partner-path", "aws.partner/example.com/9876/events"),
        ("unknown-name", "NotAValidSource"),
        ("has-a-space", "not a source"),
        ("single-character", "p"),
    ];
    for (case, source) in sources {
        let name = format!("src-{case}");
        let reply = emulator
            .call(
                "CreateChannel",
                &create_channel(&name, source, destinations.clone()),
            )
            .await;
        reply.expect_error(case, 400, "InvalidSourceException");
        assert!(
            reply.message().contains(source),
            "case {case}: the refusal must name the Source it refused, got {:?}",
            reply.message()
        );
        assert!(
            !has_channel_named(&emulator, HOME_REGION, &name),
            "case {case}: a refused CreateChannel must persist no channel"
        );
    }

    // "The number of tags per trail, event data store, dashboard, or channel has
    // exceeded the permitted amount. Currently, the limit is 50."
    //
    // An accepted row consumes its Region's one `Custom` channel, so each row runs
    // in its own Region; a refused row persists nothing and could share one, but
    // keeping every row's Region distinct keeps the rows independent of each other.
    let tag = |index: usize| json!({ "Key": format!("k{index}"), "Value": "v" });
    let tag_cases: Vec<(&str, &str, Vec<Value>, Option<&str>)> = vec![
        ("no-tags", "us-east-2", vec![], None),
        ("one-tag", "eu-central-1", vec![tag(0)], None),
        ("at-the-limit", "eu-west-2", (0..50).map(tag).collect(), None),
        (
            "one-past-the-limit",
            "ap-northeast-1",
            (0..51).map(tag).collect(),
            Some("TagsLimitExceededException"),
        ),
        (
            "duplicate-key",
            "sa-east-1",
            vec![
                json!({ "Key": "env", "Value": "a" }),
                json!({ "Key": "env", "Value": "b" }),
            ],
            Some("InvalidTagParameterException"),
        ),
    ];
    for (case, region, tags, expected) in tag_cases {
        let in_region = seed_store(&emulator, region, "eds-tags", Enabled);
        let name = format!("tags-{case}");
        let mut body = create_channel(
            &name,
            "Custom",
            vec![destination(EVENT_DATA_STORE, &in_region)],
        );
        body["Tags"] = Value::Array(tags.clone());
        let reply = emulator.call_in("CreateChannel", region, &body).await;
        match expected {
            None => {
                let created = reply.expect_ok(case);
                let reported = created.get("Tags").cloned();
                let expected_tags = (!tags.is_empty()).then(|| Value::Array(tags.clone()));
                assert_eq!(
                    reported, expected_tags,
                    "case {case}: expected the requested Tags back, got {created:?}"
                );
            }
            Some(code) => {
                reply.expect_error(case, 400, code);
                assert!(
                    !has_channel_named(&emulator, region, &name),
                    "case {case}: a refused CreateChannel must persist no channel"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_created_channels_tags_read_back_and_remove_through_the_tag_operations() {
    let emulator = Emulator::new();

    // `ListTags` reports the tags of "the specified trails, event data stores,
    // dashboards, or channels" and `RemoveTags` removes them from the same four
    // kinds, so the tags `CreateChannel` persisted are reachable through both:
    // a channel the registry holds is a tagged resource like any other. The rule
    // generalises over how many tags the channel carries, so the rows differ along
    // that; each row runs in its own Region because a Region holds at most one
    // `Custom` channel.
    let tag = |index: usize| json!({ "Key": format!("k{index}"), "Value": format!("v{index}") });
    let cases: Vec<(&str, &str, Vec<Value>)> = vec![
        ("no-tags", HOME_REGION, vec![]),
        ("one-tag", OTHER_REGION, vec![tag(0)]),
        ("three-tags", "eu-west-3", (0..3).map(tag).collect()),
    ];

    for (case, region, tags) in cases {
        let store = seed_store(&emulator, region, "eds-tag-reads", Enabled);
        let mut body = create_channel(
            case,
            "Custom",
            vec![destination(EVENT_DATA_STORE, &store)],
        );
        body["Tags"] = Value::Array(tags.clone());
        let created = emulator.call_in("CreateChannel", region, &body).await;
        let arn = string_member(created.expect_ok(case), "ChannelArn");

        let listed = emulator
            .call_in(
                "ListTags",
                region,
                &json!({ "ResourceIdList": [arn.clone()] }),
            )
            .await;
        let listed = listed.expect_ok(case);
        assert_eq!(
            listed.get("ResourceTagList"),
            Some(&json!([{ "ResourceId": arn, "TagsList": tags }])),
            "case {case}: expected the channel's stored tags, got {listed:?}"
        );

        // Removing the first key leaves the rest; an empty tag set has nothing to
        // remove, so that row only pins that the channel is found at all.
        let removed: Vec<Value> = tags.iter().take(1).cloned().collect();
        let remaining: Vec<Value> = tags.iter().skip(1).cloned().collect();
        let reply = emulator
            .call_in(
                "RemoveTags",
                region,
                &json!({ "ResourceId": arn.clone(), "TagsList": removed }),
            )
            .await;
        reply.expect_ok(case);

        let listed = emulator
            .call_in(
                "ListTags",
                region,
                &json!({ "ResourceIdList": [arn.clone()] }),
            )
            .await;
        let listed = listed.expect_ok(case);
        assert_eq!(
            listed.get("ResourceTagList"),
            Some(&json!([{ "ResourceId": arn, "TagsList": remaining }])),
            "case {case}: expected the tags left after the removal, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn get_channel_refuses_a_malformed_identifier_and_an_unknown_channel() {
    let emulator = Emulator::new();
    let account = engine::EMULATOR_ACCOUNT_ID;

    // Every value here satisfies `ChannelArn`'s modelled
    // `@pattern ^[a-zA-Z0-9._/\-:]+$` and `@length 3..256`, so the handler is what
    // refuses it. An ARN is the only form with a format to get wrong, so an `arn:`
    // value that is not a channel ARN is `ChannelARNInvalidException`, while
    // anything else is read as a channel id — and an id CloudTrail never minted
    // simply names no channel, which is `ChannelNotFoundException` (the shared
    // reading in `state::channels`, also pinned by `channel_deletion.rs`).
    let cases: Vec<(&str, String, u16, &str)> = vec![
        (
            // The recorded instance: a CloudTrail ARN with no `channel/` resource.
            "no-channel-resource",
            format!("arn:aws:cloudtrail:{OTHER_REGION}:{account}:channelwrong"),
            400,
            "ChannelARNInvalidException",
        ),
        (
            "another-cloudtrail-resource-type",
            format!("arn:aws:cloudtrail:{HOME_REGION}:{account}:trail/my-trail"),
            400,
            "ChannelARNInvalidException",
        ),
        (
            "another-service",
            format!("arn:aws:logs:{HOME_REGION}:{account}:channel/{UNKNOWN_UUID}"),
            400,
            "ChannelARNInvalidException",
        ),
        (
            "channel-arn-with-no-id",
            format!("arn:aws:cloudtrail:{HOME_REGION}:{account}:channel/"),
            400,
            "ChannelARNInvalidException",
        ),
        (
            "channel-arn-with-no-region",
            format!("arn:aws:cloudtrail::{account}:channel/{UNKNOWN_UUID}"),
            400,
            "ChannelARNInvalidException",
        ),
        (
            "channel-id-not-a-minted-uuid",
            format!("arn:aws:cloudtrail:{HOME_REGION}:{account}:channel/not-a-uuid"),
            400,
            "ChannelNotFoundException",
        ),
        (
            "bare-value-that-is-not-a-uuid",
            "my-channel".to_string(),
            400,
            "ChannelNotFoundException",
        ),
        (
            "well-formed-arn-no-such-channel",
            format!("arn:aws:cloudtrail:{HOME_REGION}:{account}:channel/{UNKNOWN_UUID}"),
            400,
            "ChannelNotFoundException",
        ),
        (
            "well-formed-uuid-no-such-channel",
            UNKNOWN_UUID.to_string(),
            400,
            "ChannelNotFoundException",
        ),
        (
            // Well formed, and the registry holds only this account's channels.
            "another-accounts-channel",
            format!("arn:aws:cloudtrail:{HOME_REGION}:210987654321:channel/{UNKNOWN_UUID}"),
            400,
            "ChannelNotFoundException",
        ),
    ];

    for (case, identifier, status, code) in cases {
        let reply = emulator
            .call("GetChannel", &json!({ "Channel": identifier }))
            .await;
        reply.expect_error(case, status, code);
        assert!(
            reply.message().contains(&identifier),
            "case {case}: the refusal must name the identifier it refused, got {:?}",
            reply.message()
        );
    }
}

