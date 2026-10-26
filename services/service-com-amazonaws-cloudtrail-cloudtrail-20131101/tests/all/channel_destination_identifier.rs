// SPDX-License-Identifier: CC0-1.0

//! How a channel destination's `Location` is read, and how each way of failing to read
//! it is refused — over the wire through the shared harness (`tests/all/emulator.rs`),
//! so what is asserted is what a client sees: the HTTP status, the error code and the
//! message.
//!
//! RULE: a `Destination$Location` of a `EVENT_DATA_STORE` destination names an event
//! data store in either of the two forms the service admits — the store's ARN, or the
//! bare id of one — and a value it cannot resolve is classified by the FORM the caller
//! wrote:
//!
//! * a value WRITTEN as an ARN (the `arn:` first field every ARN carries) that is not a
//!   well-formed event data store ARN is a malformed ARN:
//!   `EventDataStoreARNInvalidException` (HTTP 400), naming the value as given;
//! * a value in NEITHER form — not written as an ARN, and not the id of a store this
//!   service could have minted — is no event data store identifier at all:
//!   `InvalidParameterException` (HTTP 400), with the fixed sentence that states the two
//!   forms the member admits and names neither the value nor the channel;
//! * a readable identifier of either form that names no store the channel's Region can
//!   log to is an ABSENT store: `EventDataStoreNotFoundException` (HTTP 400, the status
//!   the recorded service answers, not the shape's modelled 404), naming the ARN the
//!   identifier addressed — the ARN as written, or the one a bare id completes to;
//! * a readable identifier naming a store that is not usable is
//!   `InactiveEventDataStoreException` (HTTP 400);
//! * and a readable identifier naming a usable store of the channel's Region is
//!   ACCEPTED, in whichever form it was written.
//!
//! The classifier is the registry's shared one (`state::channels`), so every row runs
//! through BOTH operations that take a `Destinations` list, `CreateChannel` and
//! `UpdateChannel`, and an accepted row is read back through `GetChannel` and
//! `ListChannels`.
//!
//! The event data stores a channel logs to are seeded into the shared registry the way
//! `channel_lifecycle.rs` seeds them (`CreateEventDataStore` is still a scaffold stub
//! with its own issue).

use serde_json::{json, Value};

use service_com_amazonaws_cloudtrail_cloudtrail_20131101::state as control_plane;
use service_com_amazonaws_cloudtrail_cloudtrail_20131101::state::EventDataStoreState::{
    Enabled, PendingDeletion,
};

// The emulator-under-test harness is shared by this crate's suites; see
// `tests/all/emulator.rs`.
use super::emulator::{Emulator, HOME_REGION};

/// `Destination$Type` for a CloudTrail Lake integration channel.
const EVENT_DATA_STORE: &str = "EVENT_DATA_STORE";

/// The status every refusal in this suite renders with: the three malformed-input
/// errors carry `@httpError 400`, and the recorded service answers 400 for
/// `EventDataStoreNotFoundException` too, though the model annotates it 404.
const CLIENT_ERROR: u16 = 400;

/// A `UUID` that no seeded event data store carries, for the absent-store rows.
const UNKNOWN_UUID: &str = "85998324-19c3-4197-86cf-3a0a481d480a";

/// The fixed sentence a `Location` in neither identifier form is refused with. Its
/// whole text is fixed: no part of it comes from the request or from state.
const NOT_AN_IDENTIFIER_MESSAGE: &str =
    "The EventDataStore value must be a valid EventDataStore ARN or ID.";

/// The resource prefix an event data store ARN carries before the store's id, from the
/// ARN format the model doc for `AddTagsRequest$ResourceId` publishes
/// (`arn:aws:cloudtrail:us-east-2:123456789012:eventdatastore/EXAMPLE-…`). Written here
/// rather than reused from the crate so these cases do not depend on the code they
/// check.
const STORE_RESOURCE_PREFIX: &str = "eventdatastore/";

/// An event data store id in the modelled `UUID` form (`8-4-4-4-12` lowercase hex),
/// built from `seed` so each seeded store carries its own id.
fn store_id(seed: usize) -> String {
    format!("{seed:08x}-0000-4000-8000-000000000000")
}

/// Seed an event data store in `status` and hand back its id — the same map, keyed the
/// same way, that `CreateEventDataStore` will write once it is implemented.
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
    id
}

/// The event data store ARN of `id` in `region`, in the published format.
fn store_arn(region: &str, id: &str) -> String {
    format!("arn:aws:cloudtrail:{region}:{}:{STORE_RESOURCE_PREFIX}{id}", engine::EMULATOR_ACCOUNT_ID)
}

/// The store `id` of `region` written in the identifier form `form` names: its ARN,
/// its bare id, or the resource-prefixed id suffix of its ARN.
fn written_as(form: &str, region: &str, id: &str) -> String {
    match form {
        "arn" => store_arn(region, id),
        "bare-id" => id.to_string(),
        "resource-prefixed-id" => format!("{STORE_RESOURCE_PREFIX}{id}"),
        other => unreachable!("case {other}: no such identifier form"),
    }
}

/// One `Destination` entry.
fn destination(kind: &str, location: &str) -> Value {
    json!({ "Type": kind, "Location": location })
}

/// A `CreateChannel` request body.
fn create_channel(name: &str, destinations: Vec<Value>) -> Value {
    json!({ "Name": name, "Source": "Custom", "Destinations": destinations })
}

/// The `Name`s `ListChannels` reports for `region`.
async fn listed_channel_names(emulator: &Emulator, region: &str) -> Vec<String> {
    let listed = emulator.call_in("ListChannels", region, &json!({})).await;
    listed
        .expect_ok("ListChannels")
        .get("Channels")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    item.get("Name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// How a destination `Location` the service cannot resolve is refused: the error code
/// and the message, as a function of the store id the Region holds.
#[derive(Debug)]
struct Expected {
    code: &'static str,
    message: String,
}

#[tokio::test]
async fn a_destination_location_that_resolves_to_no_store_is_refused_by_the_form_it_was_written_in()
{
    let emulator = Emulator::new();
    let active = seed_store(&emulator, HOME_REGION, Enabled);
    let account = engine::EMULATOR_ACCOUNT_ID;

    // A channel to point `UpdateChannel` at, so every row below runs through both
    // operations that take a `Destinations` list.
    let created = emulator
        .call(
            "CreateChannel",
            &create_channel(
                "destination-forms",
                vec![destination(EVENT_DATA_STORE, &store_arn(HOME_REGION, &active))],
            ),
        )
        .await;
    let channel_arn = created.expect_ok("the channel under update")["ChannelArn"]
        .as_str()
        .expect("CreateChannel reports the ARN it minted")
        .to_string();

    let not_an_identifier = || Expected {
        code: "InvalidParameterException",
        message: NOT_AN_IDENTIFIER_MESSAGE.to_string(),
    };
    let arn_invalid = |location: &str| Expected {
        code: "EventDataStoreARNInvalidException",
        message: format!("The event data store ARN {location} format is not valid."),
    };
    let not_found = |addressed: String| Expected {
        code: "EventDataStoreNotFoundException",
        message: format!("The event data store {addressed} was not found."),
    };

    // The rows differ along the FORM of the value — written as an ARN or not — and,
    // within each form, along which part of it is wrong: the resource type, the vendor,
    // the account, the field count, the id's spelling, the Region it names, and whether
    // the store it addresses exists. Every `Location` here satisfies the member's
    // modelled `@length 3..1024` and `@pattern ^[a-zA-Z0-9._/\-:*]+$`, so the boundary
    // validator passes it and the handler owns the refusal.
    let rows: Vec<(&str, String, Expected)> = vec![
        // ---- in NEITHER form: no event data store identifier at all ----
        (
            // The recorded instance.
            "bare-value-that-is-no-identifier",
            "not-an-eds-arn".to_string(),
            not_an_identifier(),
        ),
        (
            "bare-value-naming-a-store-by-name",
            format!("lake-{active}"),
            not_an_identifier(),
        ),
        (
            "bare-value-one-uuid-group-short",
            UNKNOWN_UUID
                .rsplit_once('-')
                .expect("a UUID carries hyphen-separated groups")
                .0
                .to_string(),
            not_an_identifier(),
        ),
        (
            // The modelled `UUID` is lowercase hex, so this spells no minted id.
            "bare-value-in-upper-case-hex",
            UNKNOWN_UUID.to_uppercase(),
            not_an_identifier(),
        ),
        (
            "resource-prefix-with-an-id-that-is-no-uuid",
            format!("{STORE_RESOURCE_PREFIX}not-a-uuid"),
            not_an_identifier(),
        ),
        (
            // Colons alone do not make an ARN: the first field has to be `arn`.
            "colons-but-no-arn-first-field",
            format!("aws:cloudtrail:{HOME_REGION}:{account}:{STORE_RESOURCE_PREFIX}{active}"),
            not_an_identifier(),
        ),
        (
            "the-destination-type-as-a-location",
            EVENT_DATA_STORE.to_string(),
            not_an_identifier(),
        ),
        // ---- WRITTEN as an ARN, but not a well-formed event data store ARN ----
        (
            // The recorded instance of this class.
            "arn-resource-type-misspelled",
            format!("arn:aws:cloudtrail:{HOME_REGION}:{account}:eventdatastorewrong/{UNKNOWN_UUID}"),
            arn_invalid(&format!(
                "arn:aws:cloudtrail:{HOME_REGION}:{account}:eventdatastorewrong/{UNKNOWN_UUID}"
            )),
        ),
        (
            "arn-of-another-service",
            format!("arn:aws:logs:{HOME_REGION}:{account}:{STORE_RESOURCE_PREFIX}{active}"),
            arn_invalid(&format!(
                "arn:aws:logs:{HOME_REGION}:{account}:{STORE_RESOURCE_PREFIX}{active}"
            )),
        ),
        (
            "arn-with-an-id-that-is-no-uuid",
            format!("arn:aws:cloudtrail:{HOME_REGION}:{account}:{STORE_RESOURCE_PREFIX}not-a-uuid"),
            arn_invalid(&format!(
                "arn:aws:cloudtrail:{HOME_REGION}:{account}:{STORE_RESOURCE_PREFIX}not-a-uuid"
            )),
        ),
        (
            "arn-whose-account-is-not-twelve-digits",
            format!("arn:aws:cloudtrail:{HOME_REGION}:12345:{STORE_RESOURCE_PREFIX}{active}"),
            arn_invalid(&format!(
                "arn:aws:cloudtrail:{HOME_REGION}:12345:{STORE_RESOURCE_PREFIX}{active}"
            )),
        ),
        (
            "arn-missing-fields",
            format!("arn:aws:cloudtrail:{STORE_RESOURCE_PREFIX}{active}"),
            arn_invalid(&format!("arn:aws:cloudtrail:{STORE_RESOURCE_PREFIX}{active}")),
        ),
        // ---- readable, and naming no store the channel's Region can log to ----
        (
            // An ARN is named as the request wrote it.
            "arn-naming-no-store",
            store_arn(HOME_REGION, UNKNOWN_UUID),
            not_found(store_arn(HOME_REGION, UNKNOWN_UUID)),
        ),
        (
            // A bare id carries no Region, so it is named by the ARN it completes to
            // in the Region the call addresses.
            "bare-id-naming-no-store",
            UNKNOWN_UUID.to_string(),
            not_found(store_arn(HOME_REGION, UNKNOWN_UUID)),
        ),
        (
            "resource-prefixed-id-naming-no-store",
            format!("{STORE_RESOURCE_PREFIX}{UNKNOWN_UUID}"),
            not_found(store_arn(HOME_REGION, UNKNOWN_UUID)),
        ),
        (
            // The store EXISTS under this id, but the ARN names another Region, and a
            // channel logs only to a store of its own Region.
            "arn-naming-another-region",
            store_arn("eu-central-1", &active),
            not_found(store_arn("eu-central-1", &active)),
        ),
    ];

    for (case, location, expected) in rows {
        let bad = destination(EVENT_DATA_STORE, &location);
        // Alone, and beside a destination that WOULD be accepted: one unresolvable
        // destination refuses the whole call.
        for (how, destinations) in [
            ("alone", vec![bad.clone()]),
            (
                "beside-a-good-one",
                vec![
                    destination(EVENT_DATA_STORE, &store_arn(HOME_REGION, &active)),
                    bad.clone(),
                ],
            ),
        ] {
            for operation in ["CreateChannel", "UpdateChannel"] {
                let case = format!("{case}/{how}/{operation}");
                let name = format!("ch-{case}").replace(['/'], "-");
                let body = if operation == "CreateChannel" {
                    create_channel(&name, destinations.clone())
                } else {
                    json!({ "Channel": channel_arn, "Destinations": destinations.clone() })
                };
                let refused = emulator.call(operation, &body).await;
                refused.expect_error(&case, CLIENT_ERROR, expected.code);
                assert_eq!(
                    refused.message(),
                    expected.message,
                    "case {case}: expected the refusal {:?}, got {:?}",
                    expected.message,
                    refused.message()
                );
            }
        }
    }

    // Nothing above persisted or changed anything: the Region still holds the one
    // channel it was created with, logging to the one store it was created with.
    assert_eq!(
        listed_channel_names(&emulator, HOME_REGION).await,
        vec!["destination-forms".to_string()],
        "case after-the-refusals: a refused CreateChannel must create no channel"
    );
    let after = emulator
        .call("GetChannel", &json!({ "Channel": channel_arn }))
        .await;
    assert_eq!(
        after.expect_ok("after-the-refusals")["Destinations"],
        json!([destination(EVENT_DATA_STORE, &store_arn(HOME_REGION, &active))]),
        "case after-the-refusals: a refused UpdateChannel must change nothing, got {after:?}"
    );
}

#[tokio::test]
async fn a_destination_location_in_either_identifier_form_names_the_same_store() {
    let emulator = Emulator::new();

    // Each row writes the SAME store a different way: its ARN, its bare id, and the
    // resource-prefixed id suffix of its ARN. Each runs in its own Region, because
    // `Custom` is the one source this emulator serves and a Region logs at most one
    // channel per source.
    let rows = [
        ("arn", "us-west-2"),
        ("bare-id", "eu-west-1"),
        ("resource-prefixed-id", "ap-northeast-1"),
    ];

    for (case, region) in rows {
        let id = seed_store(&emulator, region, Enabled);
        let location = written_as(case, region, &id);
        let name = format!("ch-{case}");
        let created = emulator
            .call_in(
                "CreateChannel",
                region,
                &create_channel(&name, vec![destination(EVENT_DATA_STORE, &location)]),
            )
            .await;
        let created = created.expect_ok(case);
        assert_eq!(
            created["Destinations"],
            json!([destination(EVENT_DATA_STORE, &location)]),
            "case {case}: expected the requested Destinations back, got {created:?}"
        );
        let channel_arn = created["ChannelArn"]
            .as_str()
            .expect("CreateChannel reports the ARN it minted")
            .to_string();

        // Round trip: the destination the create accepted is what a later read reports.
        let fetched = emulator
            .call_in("GetChannel", region, &json!({ "Channel": channel_arn }))
            .await;
        assert_eq!(
            fetched.expect_ok(case)["Destinations"],
            json!([destination(EVENT_DATA_STORE, &location)]),
            "case {case}: expected GetChannel to report the stored destination, got {fetched:?}"
        );
        assert_eq!(
            listed_channel_names(&emulator, region).await,
            vec![name.clone()],
            "case {case}: expected {region} to hold the created channel"
        );

        // And `UpdateChannel` resolves the same store written the OTHER way round: a
        // row that created by ARN updates by bare id, and the reverse.
        let other = if location.starts_with("arn:") {
            id.clone()
        } else {
            store_arn(region, &id)
        };
        let updated = emulator
            .call_in(
                "UpdateChannel",
                region,
                &json!({
                    "Channel": channel_arn,
                    "Destinations": [destination(EVENT_DATA_STORE, &other)],
                }),
            )
            .await;
        assert_eq!(
            updated.expect_ok(case)["Destinations"],
            json!([destination(EVENT_DATA_STORE, &other)]),
            "case {case}: expected the updated destination back, got {updated:?}"
        );
    }
}

#[tokio::test]
async fn a_destination_location_naming_an_unusable_store_is_refused_in_either_form() {
    let emulator = Emulator::new();
    let deleting = seed_store(&emulator, HOME_REGION, PendingDeletion);

    // A store awaiting deletion is the one status the modelled
    // `InactiveEventDataStoreException` reports, and the status is read from the STORE —
    // so it answers for every form the identifier is written in. The refusal names the
    // ARN the identifier addressed.
    let rows = ["arn", "bare-id", "resource-prefixed-id"];

    for case in rows {
        let location = written_as(case, HOME_REGION, &deleting);
        let refused = emulator
            .call(
                "CreateChannel",
                &create_channel(
                    &format!("ch-inactive-{case}"),
                    vec![destination(EVENT_DATA_STORE, &location)],
                ),
            )
            .await;
        refused.expect_error(case, CLIENT_ERROR, "InactiveEventDataStoreException");
        assert_eq!(
            refused.message(),
            format!(
                "The event data store {} is inactive.",
                store_arn(HOME_REGION, &deleting)
            ),
            "case {case}: expected the refusal to name the store it addressed, got {:?}",
            refused.message()
        );
    }

    assert!(
        listed_channel_names(&emulator, HOME_REGION).await.is_empty(),
        "case after-the-refusals: a refused CreateChannel must create no channel"
    );
}
