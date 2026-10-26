// SPDX-License-Identifier: CC0-1.0

//! The request bounds CreateRouterOutput applies before it stores anything, end to end
//! over HTTP.
//!
//! `tests/all/router_outputs.rs` pins what the create STORES and what the batch read
//! reports for it; the two rules pinned here are the ones that decide whether a request
//! reaches the store at all, each over a table rather than one request:
//!
//! 1. A create stores only a tag map this service would accept from a tag write — at most
//!    as many pairs as a resource keeps, a non-empty key of at most the documented key
//!    length and a value of at most the documented value length — and refuses past any of
//!    them with `BadRequestException` (400), storing nothing.
//! 2. A `MaintenanceConfiguration` the create stores must name a maintenance preference a
//!    window can be scheduled from: a `PreferredDayTime` `Time` is a time of day in
//!    24-hour form, and a string that names none is refused with `BadRequestException`
//!    (400) rather than stored.
//!
//! The rows vary what those rules generalise over: the number of pairs, the length of a
//! key and of a value, a key that names no tag, and — for the preference — the hour, the
//! minute, the weekday, the number of fields and whether each field is a number, every
//! bound driven AT its published value and one character, pair or unit past it.
//!
//! The harness, the create helper and the configuration fixtures are the ones
//! `flow_lifecycle` and `router_outputs` own, so every case drives the same request path a
//! MediaConnect client reaches — routing, restJson1 deserialization, constraint
//! validation, handler, serialization — and the tables of the two create suites cannot
//! drift apart.

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::flow_lifecycle::Emulator;
use crate::router_outputs::{
    batch_get_router_outputs, create_router_output, minted_network_interface, standard,
    stored_router_output_count,
};

/// A create request this service accepts, which each case below varies ONE member of, so
/// every refusal is about the thing that case changed.
fn create_body(name: &str, interface_arn: &str) -> Value {
    json!({
        "name": name,
        "configuration": standard(interface_arn, json!({"Rtp": {
            "destinationAddress": "198.51.100.90", "destinationPort": 5004}})),
        "maximumBitrate": 20_000_000i64,
        "routingScope": "REGIONAL",
        "tier": "OUTPUT_20",
    })
}

/// The router output the store holds for `arn`, read back over the wire.
async fn read_back(emulator: &Emulator, case: &str, arn: &str) -> Value {
    let (status, code, read) = batch_get_router_outputs(emulator, &[arn]).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "case {case:?}: the batch read of a stored output expected 200, got {status} \
         {code:?}: {read}"
    );
    read["routerOutputs"][0].clone()
}

/// A tag map of `count` distinct pairs.
fn many_tags(count: usize) -> Value {
    Value::Object(
        (0..count)
            .map(|index| (format!("key-{index}"), json!("value")))
            .collect(),
    )
}

/// A tag map of the single pair `key` -> `value`.
fn one_tag(key: String, value: &str) -> Value {
    let mut pairs = serde_json::Map::new();
    pairs.insert(key, json!(value));
    Value::Object(pairs)
}

/// A string of `length` characters, for driving a length bound.
fn filler(length: usize) -> String {
    "t".repeat(length)
}

/// `Tags` is modelled as `__mapOfString` with no bounds, so the create applies the
/// documented ones: at most 50 pairs on a resource, a key of at most 128 characters and a
/// value of at most 256 ("Tag keys can have a maximum character length of 128 characters,
/// and tag values can have a maximum length of 256 characters",
/// `smithy_query(query="doc",shape="TagResourceRequest",member="Tags")`), each driven AT its
/// published value and one character or pair past it, plus a key that names no tag.
#[tokio::test]
async fn a_create_stores_only_a_tag_map_the_service_would_accept() {
    let emulator = Emulator::new();
    let interface_arn = minted_network_interface(&emulator, "rni-ro-tag-bounds").await;

    // (case, tags, accepted).
    let rows = [
        ("no tags", json!({}), true),
        ("one tag", json!({ "stage": "test" }), true),
        (
            "as many tags as a resource may carry",
            many_tags(mc::MAX_TAGS_PER_RESOURCE),
            true,
        ),
        (
            "one tag past the bound",
            many_tags(mc::MAX_TAGS_PER_RESOURCE + 1),
            false,
        ),
        ("an empty key", one_tag(String::new(), "value"), false),
        (
            "a key at the documented length",
            one_tag(filler(mc::MAX_TAG_KEY_LENGTH), "value"),
            true,
        ),
        (
            "a key one character past it",
            one_tag(filler(mc::MAX_TAG_KEY_LENGTH + 1), "value"),
            false,
        ),
        (
            "a value at the documented length",
            one_tag("stage".to_string(), &filler(mc::MAX_TAG_VALUE_LENGTH)),
            true,
        ),
        (
            "a value one character past it",
            one_tag("stage".to_string(), &filler(mc::MAX_TAG_VALUE_LENGTH + 1)),
            false,
        ),
    ];

    // The interface the configurations name is the only router output the store does NOT
    // hold yet, so the count starts at nothing and grows by one per accepted create.
    let mut accepted_count = 0usize;
    for (case, tags, accepted) in rows {
        let mut body = create_body(&format!("ro-tags-{accepted_count}"), &interface_arn);
        body["tags"] = tags.clone();
        let (status, code, answered) = create_router_output(&emulator, body).await;
        if accepted {
            accepted_count += 1;
            assert_eq!(
                (status, code.as_deref()),
                (201, None),
                "case {case:?}: the create is accepted, got {status} {code:?}: {answered}"
            );
            let arn = answered["routerOutput"]["arn"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let stored = read_back(&emulator, case, &arn).await;
            assert_eq!(
                stored["tags"], tags,
                "case {case:?}: the stored output carries the tags the create named, got {:?}",
                stored["tags"]
            );
        } else {
            assert_eq!(
                (status, code.as_deref()),
                (400, Some("BadRequestException")),
                "case {case:?}: the create is refused, got {status} {code:?}: {answered}"
            );
            assert!(
                answered["message"]
                    .as_str()
                    .unwrap_or_default()
                    .starts_with(mc::VALIDATION_FAILED_PREFIX),
                "case {case:?}: the refusal carries this service's validation envelope, got \
                 {answered}"
            );
        }
        assert_eq!(
            stored_router_output_count(&emulator),
            accepted_count,
            "case {case:?}: a refused create stores nothing, got {} stored",
            stored_router_output_count(&emulator)
        );
    }
}

/// `PreferredDayTimeMaintenanceConfiguration$Time` is "the preferred time for maintenance
/// operations" and the window a start schedules is derived from it, so a `Time` that names
/// no time of day names no preference the service can honour. The form is the 24-hour one
/// this service documents for the same preference on a flow
/// (`smithy_query(query="doc",shape="UpdateMaintenance",member="MaintenanceStartHour")`:
/// "UTC time when the maintenance will happen. Use 24-hour HH:MM format"); `Time` is a
/// plain String in the model, so the create is what refuses a string outside it.
///
/// The accepted rows walk the day across the week — `Day` is the modelled enum, so every
/// weekday is a valid preference — and the refused rows are the ways a string fails to name
/// a time of day: an hour or a minute past its range, a field that is not a number, and too
/// few or too many fields.
#[tokio::test]
async fn a_maintenance_preference_must_name_a_time_of_day() {
    let emulator = Emulator::new();
    let interface_arn = minted_network_interface(&emulator, "rni-ro-maintenance").await;

    // (case, Day, Time, accepted).
    let rows = [
        ("midnight", "MONDAY", "00:00", true),
        ("morning", "TUESDAY", "09:30", true),
        ("the last minute of the day", "WEDNESDAY", "23:59", true),
        ("a time written out in full", "THURSDAY", "17:45:30", true),
        ("midday", "SUNDAY", "12:00", true),
        ("an hour past the day", "FRIDAY", "24:00", false),
        ("a minute past the hour", "SATURDAY", "12:60", false),
        ("not a number", "MONDAY", "noon", false),
        ("no minutes", "MONDAY", "12", false),
        ("an empty time", "MONDAY", "", false),
        ("a fourth field", "MONDAY", "12:00:00:00", false),
    ];

    let mut accepted_count = 0usize;
    for (case, day, time, accepted) in rows {
        let mut body = create_body(&format!("ro-window-{accepted_count}"), &interface_arn);
        body["maintenanceConfiguration"] = json!({"PreferredDayTime": {"day": day, "time": time}});
        let (status, code, answered) = create_router_output(&emulator, body).await;
        if accepted {
            accepted_count += 1;
            assert_eq!(
                (status, code.as_deref()),
                (201, None),
                "case {case:?}: the create is accepted, got {status} {code:?}: {answered}"
            );
            let arn = answered["routerOutput"]["arn"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let stored = read_back(&emulator, case, &arn).await;
            let preference = &stored["maintenanceConfiguration"]["PreferredDayTime"];
            assert_eq!(
                (
                    stored["maintenanceType"].as_str(),
                    preference["day"].as_str(),
                    preference["time"].as_str(),
                ),
                (
                    Some(mc::MAINTENANCE_TYPE_PREFERRED_DAY_TIME),
                    Some(day),
                    Some(time),
                ),
                "case {case:?}: MaintenanceType follows from the union member and the \
                 preference is stored as given, got {stored}"
            );
        } else {
            assert_eq!(
                (status, code.as_deref()),
                (400, Some("BadRequestException")),
                "case {case:?}: a string that names no time of day is refused, got {status} \
                 {code:?}: {answered}"
            );
        }
        assert_eq!(
            stored_router_output_count(&emulator),
            accepted_count,
            "case {case:?}: a refused create stores nothing, got {} stored",
            stored_router_output_count(&emulator)
        );
    }
}
