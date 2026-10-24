// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListTags`, the READER of every tag set this service stores.
//!
//! Every case goes over the wire through the shared harness (`tests/all/emulator.rs`):
//! an `awsJson1_1` `POST /` with this service's published `X-Amz-Target` prefix,
//! dispatched through the same `MultiServiceRouter` the production router serves, so
//! what is asserted here is what a client sees — the HTTP status, the error code and
//! the message.
//!
//! The rule under test, grounded in this service's Smithy model: a tagging call
//! resolves its ARN against the registry that HOLDS that ARN's resource kind — "the
//! tags for the specified trails, event data stores, dashboards, or channels in the
//! current Region" (`ListTags`) — and reports or writes THAT record's stored tag set,
//! so an ARN is answered out of persisted state whichever of the four kinds it names,
//! and the kind-specific not-found refusal means that registry holds no record at the
//! address the ARN pins (name or id, plus Region). `ListTags` reports one entry per ARN
//! the request names, in the order it named them, and reports only — it creates no
//! resource and writes no tag.
//!
//! Every resource here is created through its own operation and read back through
//! `ListTags`, so each row is a round trip over the wire rather than a seeded record;
//! the state registries are read directly only to assert that a read changed none of
//! them.

use serde_json::{json, Value};

use service_com_amazonaws_cloudtrail_cloudtrail_20131101::ops::types::Tag;
use service_com_amazonaws_cloudtrail_cloudtrail_20131101::state::{self, channels};

use super::emulator::{Emulator, HOME_REGION};

/// Another Region, for the rows that pin a resource to the Region its ARN carries. A
/// Region holds at most one `Custom`-source channel, so a second channel needs one.
const OTHER_REGION: &str = "us-west-2";

/// `Destination$Type` for a CloudTrail Lake integration channel, and the one source a
/// channel is created for here (`CreateChannelRequest$Source`).
const EVENT_DATA_STORE: &str = "EVENT_DATA_STORE";
/// See [`EVENT_DATA_STORE`].
const CUSTOM_SOURCE: &str = "Custom";

/// One tag set as a row writes it: each key, and the value it carries if any ("If you
/// specify a key without a value, the tag will be created with the specified key and a
/// value of null").
type TagPairs<'a> = &'a [(&'a str, Option<&'a str>)];

/// One tag set as the registry holds it: each key and the value it carries, if any.
type StoredPairs = Vec<(String, Option<String>)>;

/// A tag set as the wire carries it — the shape of a `TagsList` in a request and in a
/// `ListTags` reply alike.
fn wire_tags(pairs: TagPairs<'_>) -> Value {
    Value::Array(
        pairs
            .iter()
            .map(|(key, value)| match value {
                Some(value) => json!({"Key": key, "Value": value}),
                None => json!({"Key": key}),
            })
            .collect(),
    )
}

/// The same tag set as a snapshot of the registry holds it.
fn stored_pairs(pairs: TagPairs<'_>) -> StoredPairs {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), value.map(str::to_string)))
        .collect()
}

/// A string member of a reply body.
fn string_member(case: &str, body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case}: no {member} in {body:?}"))
        .to_string()
}

/// Create a trail in `region` carrying `tags`, and hand back its ARN.
async fn create_trail(emulator: &Emulator, region: &str, name: &str, tags: TagPairs<'_>) -> String {
    let case = format!("CreateTrail {name} in {region}");
    let reply = emulator
        .call_in(
            "CreateTrail",
            region,
            &json!({
                "Name": name,
                "S3BucketName": "list-tags-bucket",
                "TagsList": wire_tags(tags),
            }),
        )
        .await;
    string_member(&case, reply.expect_ok(&case), "TrailARN")
}

/// Create a dashboard in `region` carrying `tags`, and hand back its ARN.
async fn create_dashboard(
    emulator: &Emulator,
    region: &str,
    name: &str,
    tags: TagPairs<'_>,
) -> String {
    let case = format!("CreateDashboard {name} in {region}");
    let reply = emulator
        .call_in(
            "CreateDashboard",
            region,
            &json!({"Name": name, "TagsList": wire_tags(tags)}),
        )
        .await;
    string_member(&case, reply.expect_ok(&case), "DashboardArn")
}

/// Create an event data store in `region` carrying `tags`, and hand back its ARN.
async fn create_event_data_store(
    emulator: &Emulator,
    region: &str,
    name: &str,
    tags: TagPairs<'_>,
) -> String {
    let case = format!("CreateEventDataStore {name} in {region}");
    let reply = emulator
        .call_in(
            "CreateEventDataStore",
            region,
            &json!({"Name": name, "TagsList": wire_tags(tags)}),
        )
        .await;
    string_member(&case, reply.expect_ok(&case), "EventDataStoreArn")
}

/// Create a channel in `region` carrying `tags`, logging to a new event data store of
/// that Region, and hand back its ARN. A Region holds at most one `Custom` channel, so
/// one call per Region.
async fn create_channel(
    emulator: &Emulator,
    region: &str,
    name: &str,
    tags: TagPairs<'_>,
) -> String {
    let destination = create_event_data_store(emulator, region, &format!("{name}-store"), &[]).await;
    let case = format!("CreateChannel {name} in {region}");
    let reply = emulator
        .call_in(
            "CreateChannel",
            region,
            &json!({
                "Name": name,
                "Source": CUSTOM_SOURCE,
                "Destinations": [{"Type": EVENT_DATA_STORE, "Location": destination}],
                "Tags": wire_tags(tags),
            }),
        )
        .await;
    string_member(&case, reply.expect_ok(&case), "ChannelArn")
}

/// The channel id inside a channel ARN (`…:channel/<uuid>`), so a case can address the
/// same id in another Region.
fn channel_id_of(channel_arn: &str) -> String {
    channel_arn
        .split("channel/")
        .nth(1)
        .unwrap_or_else(|| panic!("{channel_arn:?} is not a channel ARN"))
        .to_string()
}

/// The `(ResourceId, TagsList)` pairs a `ListTags` reply carries, in reply order. A
/// `ResourceTag` sent without a `TagsList` reads as the empty tag set, which is what a
/// resource nobody has tagged carries.
fn reported_tags(case: &str, body: &Value) -> Vec<(String, Value)> {
    body.get("ResourceTagList")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case}: no ResourceTagList in {body:?}"))
        .iter()
        .map(|entry| {
            (
                entry
                    .get("ResourceId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                entry.get("TagsList").cloned().unwrap_or(json!([])),
            )
        })
        .collect()
}

/// The tag set `ListTags` reports for one ARN.
async fn listed_tags(emulator: &Emulator, case: &str, resource_id: &str) -> Value {
    let reply = emulator
        .call("ListTags", &json!({"ResourceIdList": [resource_id]}))
        .await;
    let body = reply.expect_ok(case);
    let reported = reported_tags(case, body);
    assert_eq!(
        reported.len(),
        1,
        "case {case}: expected one entry for one ARN, got {reported:?}"
    );
    assert_eq!(
        reported[0].0, resource_id,
        "case {case}: expected the entry to name the ARN asked for, got {reported:?}"
    );
    reported[0].1.clone()
}

/// The tag set the channel registry itself holds for the channel of `id` in `region` —
/// the store the handlers read, for the cases that assert a refusal wrote nothing.
/// `None` is the registry holding no such channel, which is a different fact from a
/// channel carrying no tag.
fn stored_channel_tags(emulator: &Emulator, region: &str, id: &str) -> Option<StoredPairs> {
    let state = emulator.state();
    let store = state.lock();
    store
        .channels
        .get(&(id.to_string(), region.to_string()))
        .map(|record| {
            record
                .tags
                .iter()
                .map(|tag| (tag.key.clone(), tag.value.clone()))
                .collect()
        })
}

/// Every taggable record the store holds, keyed by kind and address, with its tag set —
/// the whole of what a tagging call may report, for the case that asserts a read
/// changes none of it.
fn taggable_snapshot(emulator: &Emulator) -> Vec<(String, StoredPairs)> {
    let pairs = |tags: &[Tag]| -> StoredPairs {
        tags.iter()
            .map(|tag| (tag.key.clone(), tag.value.clone()))
            .collect()
    };
    let state = emulator.state();
    let store = state.lock();
    let mut snapshot: Vec<(String, StoredPairs)> = Vec::new();
    for ((name, region), record) in &store.trails {
        snapshot.push((format!("trail {region} {name}"), pairs(&record.tags)));
    }
    for ((name, region), record) in &store.dashboards {
        snapshot.push((format!("dashboard {region} {name}"), pairs(&record.tags)));
    }
    for (id, record) in &store.event_data_stores {
        snapshot.push((
            format!("eventdatastore {} {id}", record.region),
            pairs(&record.tags),
        ));
    }
    for ((id, region), record) in &store.channels {
        snapshot.push((format!("channel {region} {id}"), pairs(&record.tags)));
    }
    snapshot
}

/// RULE: `ListTags` reports the stored tag set of the resource each ARN names, read out
/// of the registry that holds that ARN's KIND — a trail, a dashboard, an event data
/// store or a channel — and one call reports every ARN it was given, in the order it
/// was given them.
#[tokio::test]
async fn list_tags_reports_the_stored_tag_set_of_every_taggable_kind() {
    let emulator = Emulator::new();

    let trail_tags: TagPairs<'_> = &[("team", Some("security")), ("stage", None)];
    let dashboard_tags: TagPairs<'_> = &[("owner", Some("alice"))];
    let store_tags: TagPairs<'_> = &[("a", Some("1")), ("b", Some("2")), ("c", Some("3"))];
    let channel_tags: TagPairs<'_> = &[("ingest", Some("custom")), ("stage", None)];

    let trail = create_trail(&emulator, HOME_REGION, "reader-trail", trail_tags).await;
    let untagged_trail = create_trail(&emulator, OTHER_REGION, "reader-trail-west", &[]).await;
    let dashboard =
        create_dashboard(&emulator, HOME_REGION, "reader_dashboard", dashboard_tags).await;
    let store = create_event_data_store(&emulator, HOME_REGION, "reader-store", store_tags).await;
    let channel = create_channel(&emulator, HOME_REGION, "reader-channel", channel_tags).await;
    let untagged_channel =
        create_channel(&emulator, OTHER_REGION, "reader-channel-west", &[]).await;

    // The rows differ in the KIND the ARN names, the Region it pins, the SIZE of the
    // stored tag set (none, one, two, three) and whether a tag carries a value.
    let rows: Vec<(&str, &String, Value)> = vec![
        ("a trail's tag set", &trail, wire_tags(trail_tags)),
        (
            "a trail nobody tagged, in another Region",
            &untagged_trail,
            json!([]),
        ),
        (
            "a dashboard's tag set",
            &dashboard,
            wire_tags(dashboard_tags),
        ),
        ("an event data store's tag set", &store, wire_tags(store_tags)),
        ("a channel's tag set", &channel, wire_tags(channel_tags)),
        (
            "a channel nobody tagged, in another Region",
            &untagged_channel,
            json!([]),
        ),
    ];

    for (case, resource_id, expected) in &rows {
        let actual = listed_tags(&emulator, case, resource_id).await;
        assert_eq!(
            &actual, expected,
            "case {case}: expected the tag set {expected:?}, got {actual:?}"
        );
    }

    // One call naming every ARN at once: one entry per ARN, in request order, each with
    // its own resource's tag set.
    let case = "every taggable kind in one call";
    let ids: Vec<&String> = rows.iter().map(|(_, resource_id, _)| *resource_id).collect();
    let reply = emulator
        .call("ListTags", &json!({"ResourceIdList": ids}))
        .await;
    let reported = reported_tags(case, reply.expect_ok(case));
    let expected: Vec<(String, Value)> = rows
        .iter()
        .map(|(_, resource_id, tags)| ((*resource_id).clone(), tags.clone()))
        .collect();
    assert_eq!(
        reported, expected,
        "case {case}: expected {expected:?}, got {reported:?}"
    );
}

/// RULE: a channel's tag set is the one the channel registry holds, so a tag write
/// reaches that record and `ListTags` reports what the write left — and a channel ARN
/// the registry holds no record for is `ChannelNotFoundException` (HTTP 400), worded
/// from the request as the recorded corpus words it ("The channel `<arn>` was not
/// found."), whatever other channels the registry holds.
#[tokio::test]
async fn a_channels_tag_set_round_trips_through_the_tagging_operations() {
    let emulator = Emulator::new();
    let channel = create_channel(&emulator, HOME_REGION, "round-trip-channel", &[]).await;
    let id = channel_id_of(&channel);

    // (case, the operation, the tags the call names, the tag set the channel then
    // carries). The rows differ in which operation writes, how many keys it names,
    // whether a key is new, an overwrite or one the channel does not carry, and
    // whether a key carries a value at all.
    let steps: Vec<(&str, &str, TagPairs<'_>, TagPairs<'_>)> = vec![
        (
            "the first AddTags",
            "AddTags",
            &[("team", Some("security"))],
            &[("team", Some("security"))],
        ),
        (
            "a second AddTags adding a key with a value and one without",
            "AddTags",
            &[("owner", Some("alice")), ("stage", None)],
            &[
                ("team", Some("security")),
                ("owner", Some("alice")),
                ("stage", None),
            ],
        ),
        (
            "an AddTags overwriting a key the channel carries",
            "AddTags",
            &[("team", Some("platform"))],
            &[
                ("team", Some("platform")),
                ("owner", Some("alice")),
                ("stage", None),
            ],
        ),
        (
            "a RemoveTags dropping one key",
            "RemoveTags",
            &[("owner", Some("alice"))],
            &[("team", Some("platform")), ("stage", None)],
        ),
        (
            "a RemoveTags naming a key the channel does not carry",
            "RemoveTags",
            &[("not-held", Some("x"))],
            &[("team", Some("platform")), ("stage", None)],
        ),
    ];

    for (case, operation, written, held) in steps {
        emulator
            .call(
                operation,
                &json!({"ResourceId": &channel, "TagsList": wire_tags(written)}),
            )
            .await
            .expect_ok(case);
        // Read back over the WIRE: the tag set a client sees after the write.
        let listed = listed_tags(&emulator, case, &channel).await;
        assert_eq!(
            listed,
            wire_tags(held),
            "case {case}: expected the tag set {:?}, got {listed:?}",
            wire_tags(held)
        );
        // And the registry the handlers read holds the same thing.
        let stored = stored_channel_tags(&emulator, HOME_REGION, &id);
        let expected = Some(stored_pairs(held));
        assert_eq!(
            stored, expected,
            "case {case}: expected the registry to hold {expected:?}, got {stored:?}"
        );
    }

    let held_before = stored_channel_tags(&emulator, HOME_REGION, &id);

    // A channel ARN the registry holds no record for: the stored channel's id in
    // another Region (a channel ARN carries the Region its channel lives in), a minted
    // id no channel carries, and an id CloudTrail never minted.
    let absent = [
        (
            "the stored channel's id in another Region",
            channels::channel_arn(OTHER_REGION, &id),
        ),
        (
            "a minted id the registry does not hold",
            channels::channel_arn(HOME_REGION, "7c9e6679-7425-40de-944b-e07fc1f90ae7"),
        ),
        (
            "an id CloudTrail never minted",
            channels::channel_arn(HOME_REGION, "not-a-minted-channel-id"),
        ),
    ];
    for (case, resource_id) in &absent {
        let reply = emulator
            .call("ListTags", &json!({"ResourceIdList": [resource_id]}))
            .await;
        reply.expect_error(case, 400, "ChannelNotFoundException");
        let expected = format!("The channel {resource_id} was not found.");
        assert_eq!(
            reply.message(),
            expected,
            "case {case}: expected the message {expected:?}, got {reply:?}"
        );
    }

    // None of those refusals touched the channel that IS stored.
    let held_after = stored_channel_tags(&emulator, HOME_REGION, &id);
    assert_eq!(
        held_after, held_before,
        "expected a refused read to leave the stored tag set alone, got {held_after:?}"
    );
}

/// RULE: `ListTags` only REPORTS — it creates no resource and changes no tag set, so a
/// repeated call reports the same thing and a call naming an ARN the registries hold no
/// record for leaves them as they were.
#[tokio::test]
async fn a_list_tags_call_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();
    let trail = create_trail(
        &emulator,
        HOME_REGION,
        "read-only-trail",
        &[("team", Some("security"))],
    )
    .await;
    let channel = create_channel(
        &emulator,
        HOME_REGION,
        "read-only-channel",
        &[("ingest", Some("custom"))],
    )
    .await;
    let store = create_event_data_store(
        &emulator,
        HOME_REGION,
        "read-only-store",
        &[("a", Some("1"))],
    )
    .await;

    let before = taggable_snapshot(&emulator);
    let case = "the first read";
    let first = emulator
        .call(
            "ListTags",
            &json!({"ResourceIdList": [&trail, &channel, &store]}),
        )
        .await;
    let first_body = first.expect_ok(case).clone();

    // A read that is refused writes nothing either: no ARN naming nothing becomes a
    // resource a later caller can see, whichever kind it names.
    for (case, resource_id, code) in [
        (
            "a channel ARN naming no channel",
            channels::channel_arn(HOME_REGION, "9f1a2b3c-4d5e-4f60-8a9b-0c1d2e3f4a5b"),
            "ChannelNotFoundException",
        ),
        (
            "a trail ARN naming no trail",
            state::trail_arn(HOME_REGION, "no-such-trail"),
            "ResourceNotFoundException",
        ),
        (
            "an event data store ARN naming no store",
            state::event_data_store_arn(HOME_REGION, "cccccccc-1111-2222-3333-444444444444"),
            "EventDataStoreNotFoundException",
        ),
    ] {
        emulator
            .call("ListTags", &json!({"ResourceIdList": [&resource_id]}))
            .await
            .expect_error(case, 400, code);
    }

    let case = "the second read";
    let second = emulator
        .call(
            "ListTags",
            &json!({"ResourceIdList": [&trail, &channel, &store]}),
        )
        .await;
    let second_body = second.expect_ok(case);
    assert_eq!(
        &first_body, second_body,
        "case {case}: expected the same reply as the first read, got {second_body:?}"
    );

    let after = taggable_snapshot(&emulator);
    assert_eq!(
        after, before,
        "expected the reads to leave every taggable record as it was, got {after:?}"
    );
}
