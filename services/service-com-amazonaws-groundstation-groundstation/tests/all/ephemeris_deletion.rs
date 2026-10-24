// SPDX-License-Identifier: CC0-1.0

//! `DeleteEphemeris` (`DELETE /ephemeris/{ephemerisId}`) end to end over HTTP, through the
//! same front door as the rest of this target's suites.
//!
//! The RULE under test: a delete REMOVES the ephemeris this account holds under the
//! `{ephemerisId}` it is addressed by and answers that id, so every later read of it, every
//! listing, every ARN-valued reference to it and every repeat of the delete answers as it
//! does for an ephemeris that never existed — EXCEPT while a contact still to be flown
//! depends on it, which is the ephemeris in force for that contact's satellite: then the
//! delete is refused with the modelled `ResourceInUseException` (HTTP 409) and the store is
//! left exactly as it was.
//!
//! So the suites below assert that rule over SETS of inputs rather than over one call:
//!
//! * a table spanning every `EphemerisData` member a request may carry (`tle`, `oem`,
//!   `azEl` — `smithy_query(query="shape", shape="EphemerisData")`), each supplied both
//!   inline and from an Amazon S3 object, differing in name, priority (each published bound
//!   of `EphemerisPriority` among them), whether the caller asked for it to be enabled,
//!   whether it names a satellite, how it is tagged and which `EphemerisStatus` it has
//!   reached — each deleted and then asserted gone from every reader;
//! * the in-use refusal over the WHOLE `ContactStatus` enum (`smithy_query(query="shape",
//!   shape="ContactStatus")`): a contact in each value in turn, flown against the satellite
//!   the ephemeris in force applies to, with the delete refused for every non-terminal
//!   status and accepted for every terminal one;
//! * the boundary of "depends on": a table of ephemerides a non-terminal contact does NOT
//!   hold — one of another satellite, one that applies to no satellite, and one of the
//!   contact's own satellite in each `EphemerisStatus` that is not in force — every one of
//!   which deletes while that contact stands;
//! * the refusal of an id that names no ephemeris, over several well-formed `Uuid`s, and of
//!   ids outside the modelled `Uuid` pattern, neither of which disturbs the store.

use serde_json::{json, Value};
use service_com_amazonaws_groundstation_groundstation::state::{
    ephemeris_in_use_message, service_state, BAD_REQUEST_ERROR_CODE, EPHEMERIS_DISABLED,
    EPHEMERIS_ENABLED, EPHEMERIS_EXPIRED, EPHEMERIS_INVALID, EPHEMERIS_NOT_FOUND,
    EPHEMERIS_VALIDATING, INVALID_REQUEST_BODY, TERMINAL_CONTACT_STATUSES,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::ephemeris_catalog::{
    az_el_inline, create_ok, onboard, s3_object, tle_inline, wait_out_validation, FUTURE, OEM_BODY,
    PAST,
};
use crate::scheduling_loop::{
    create_mission_profile, harness_with_engine, send, Reply, ABSENT_UUID, AFTER_SETTLE, REGION,
};

/// Every value of the model's `ContactStatus`.
///
/// Source: `smithy_query(query="shape", shape="ContactStatus",
/// model_path="services/service-com-amazonaws-groundstation-groundstation/smithy-model/\
/// com-amazonaws-groundstation-groundstation.json")` — all thirteen values, not only the
/// ones a request can reach today, because the rule is over the whole enum. Which of them
/// are the ends of the lifecycle is `state::TERMINAL_CONTACT_STATUSES`, asserted here to be
/// a subset of this list so the two cannot drift apart.
const CONTACT_STATUSES: &[&str] = &[
    "AVAILABLE",
    "AWS_CANCELLED",
    "AWS_FAILED",
    "CANCELLED",
    "CANCELLING",
    "COMPLETED",
    "FAILED",
    "FAILED_TO_SCHEDULE",
    "PASS",
    "POSTPASS",
    "PREPASS",
    "SCHEDULED",
    "SCHEDULING",
];

/// The window no expiration this suite stores falls outside: the first second of the epoch
/// to 2100-01-01, so a listing taken over it reports every ephemeris the store holds.
const WHOLE_WINDOW: (i64, i64) = (1, 4_102_444_800);

/// `DeleteEphemeris` for `ephemeris_id`.
async fn delete(router: &MultiServiceRouter, ephemeris_id: &str) -> Reply {
    send(router, "DELETE", &format!("/ephemeris/{ephemeris_id}"), None).await
}

/// `DescribeEphemeris` for `ephemeris_id`.
async fn describe(router: &MultiServiceRouter, ephemeris_id: &str) -> Reply {
    send(router, "GET", &format!("/ephemeris/{ephemeris_id}"), None).await
}

/// The `status` `DescribeEphemeris` reports for `ephemeris_id`.
async fn described_status(router: &MultiServiceRouter, case: &str, ephemeris_id: &str) -> String {
    let read = describe(router, ephemeris_id).await;
    assert_eq!(
        read.status, 200,
        "case {case}: expected DescribeEphemeris 200, got {} {}",
        read.status, read.body
    );
    read.string("status")
}

/// Every `ephemerisId` `ListEphemerides` reports over [`WHOLE_WINDOW`].
async fn listed_ids(router: &MultiServiceRouter, case: &str) -> Vec<String> {
    let reply = send(
        router,
        "POST",
        "/ephemerides",
        Some(json!({"startTime": WHOLE_WINDOW.0, "endTime": WHOLE_WINDOW.1})),
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "case {case}: expected ListEphemerides 200, got {} {}",
        reply.status, reply.body
    );
    reply
        .field("ephemerides")
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    item.get("ephemerisId")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// An ephemeris's ARN, `ephemeris/<ephemerisId>`: no operation answers one, and it is what
/// the tagging operations address an ephemeris by, so it is composed the way the model
/// addresses the resource — the same composition `tests/all/resource_tagging.rs` uses.
fn ephemeris_arn(ephemeris_id: &str) -> String {
    format!(
        "arn:aws:groundstation:{REGION}:{}:ephemeris/{ephemeris_id}",
        engine::emulator_account_id()
    )
}

/// `ListTagsForResource` for an ARN, which resolves only while the resource it names is
/// held. The ARN is the one `{resourceArn}` path segment, percent-encoded as an SDK sends
/// it (the label is not greedy).
async fn list_tags(router: &MultiServiceRouter, arn: &str) -> Reply {
    send(
        router,
        "GET",
        &format!("/tags/{}", arn.replace('%', "%25").replace('/', "%2F")),
        None,
    )
    .await
}

/// The ARN of an on-boarded satellite, read from the store.
///
/// Satellites are not created through this API and `GetSatellite` is still a scaffolded
/// 501, so the only reader of the ARN a contact names its satellite by is the store itself.
fn satellite_arn(engine: &engine::InMemoryEngine, satellite_id: &str) -> String {
    service_state(engine)
        .expect("this service's state store")
        .lock()
        .satellites
        .get(satellite_id)
        .expect("an on-boarded satellite")
        .satellite_arn
        .clone()
}

/// Put the stored contact into `status`.
///
/// Most `ContactStatus` values are reached by the scheduler and by the pass itself, which
/// no request drives, so the stored contact's status is set directly — as
/// `tests/all/contact_cancellation.rs` does for the same reason.
fn put_in_status(engine: &engine::InMemoryEngine, contact_id: &str, status: &str) {
    service_state(engine)
        .expect("this service's state store")
        .lock()
        .contacts
        .get_mut(contact_id)
        .expect("the reserved contact")
        .contact_status = status.to_string();
}

/// Reserve a contact flown against `satellite_arn`, and answer its id.
async fn reserve_against(
    router: &MultiServiceRouter,
    case: &str,
    profile_arn: &str,
    satellite_arn: &str,
) -> String {
    let start = 1_800_000_000_i64;
    let reserved = send(
        router,
        "POST",
        "/contact",
        Some(json!({
            "missionProfileArn": profile_arn,
            "groundStation": "Ohio 1",
            "startTime": start,
            "endTime": start + 600,
            "satelliteArn": satellite_arn,
        })),
    )
    .await;
    assert_eq!(
        reserved.status, 200,
        "case {case}: expected ReserveContact 200, got {} {}",
        reserved.status, reserved.body
    );
    reserved.string("contactId")
}

/// One row of the removal table: an ephemeris a request may store, and the
/// `EphemerisStatus` it has reached by the time it is deleted.
struct Removal {
    case: &'static str,
    body: Value,
    /// Whether the row's validation is waited out before the delete, so the rows span both
    /// an ephemeris still `VALIDATING` and one that has settled.
    settled: bool,
    status: &'static str,
    /// Whether the row's request names the on-boarded satellite.
    satellite: bool,
}

/// Every form of ephemeris a delete must remove: each member of the `EphemerisData` union
/// (`smithy_query(query="shape", shape="EphemerisData")` — `tle`, `oem`, `azEl`) supplied
/// both inline and from an Amazon S3 object, each published bound of
/// `CustomerEphemerisPriority` (`range 1..99999` — `smithy_query(query="operation",
/// operation="CreateEphemeris")`) and the absent member that takes the documented default,
/// with and without tags, with and without a satellite, and in every `EphemerisStatus` this
/// service puts a customer ephemeris in.
fn removals() -> Vec<Removal> {
    vec![
        Removal {
            case: "tle inline, enabled, tagged, priority 1",
            body: json!({
                "name": "tle inline enabled",
                "enabled": true,
                "priority": 1,
                "expirationTime": FUTURE,
                "tags": {"stage": "created"},
                "ephemeris": tle_inline(&[(1_800_000_000, 1_800_086_400)]),
            }),
            settled: true,
            status: EPHEMERIS_ENABLED,
            satellite: true,
        },
        Removal {
            case: "tle from s3, no priority member, untagged",
            body: json!({
                "name": "tle from s3",
                "enabled": true,
                "expirationTime": FUTURE,
                "ephemeris": {"tle": {"s3Object": s3_object("tle/doomed.txt")}},
            }),
            settled: true,
            status: EPHEMERIS_ENABLED,
            satellite: false,
        },
        Removal {
            case: "oem inline, not enabled, priority 99999",
            body: json!({
                "name": "oem inline disabled",
                "enabled": false,
                "priority": 99999,
                "expirationTime": FUTURE,
                "ephemeris": {"oem": {"oemData": OEM_BODY}},
            }),
            settled: true,
            status: EPHEMERIS_DISABLED,
            satellite: true,
        },
        Removal {
            case: "oem from s3, asked for neither enabled nor disabled",
            body: json!({
                "name": "oem from s3",
                "expirationTime": FUTURE,
                "ephemeris": {"oem": {"s3Object": s3_object("oem/doomed.oem")}},
            }),
            settled: true,
            status: EPHEMERIS_DISABLED,
            satellite: false,
        },
        Removal {
            case: "azEl inline, still validating",
            body: json!({
                "name": "azEl validating",
                "enabled": true,
                "priority": 42,
                "expirationTime": FUTURE,
                "tags": {"stage": "created", "owner": "flight ops"},
                "ephemeris": az_el_inline(&[(1_800_000_000, 1_800_086_400)]),
            }),
            settled: false,
            status: EPHEMERIS_VALIDATING,
            satellite: true,
        },
        Removal {
            case: "azEl from s3, still validating",
            body: json!({
                "name": "azEl from s3",
                "enabled": false,
                "expirationTime": FUTURE,
                "ephemeris": {"azEl": {
                    "groundStation": "Ohio 1",
                    "data": {"s3Object": s3_object("azel/doomed.json")},
                }},
            }),
            settled: false,
            status: EPHEMERIS_VALIDATING,
            satellite: false,
        },
        Removal {
            case: "a tle validity range that runs backwards",
            body: json!({
                "name": "backwards tle",
                "enabled": true,
                "expirationTime": FUTURE,
                "ephemeris": tle_inline(&[(1_800_086_400, 1_800_000_000)]),
            }),
            settled: true,
            status: EPHEMERIS_INVALID,
            satellite: true,
        },
        Removal {
            case: "an expiration already in the past",
            body: json!({
                "name": "expired tle",
                "enabled": true,
                "expirationTime": PAST,
                "ephemeris": tle_inline(&[(1_800_000_000, 1_800_086_400)]),
            }),
            settled: true,
            status: EPHEMERIS_EXPIRED,
            satellite: true,
        },
    ]
}

#[tokio::test]
async fn a_deleted_ephemeris_is_gone_from_every_read_and_every_reference() {
    let (router, engine) = harness_with_engine();
    let satellites = onboard(&engine, "25994=Ohio 1");
    let satellite_id = satellites
        .first()
        .expect("the onboarding stores one satellite")
        .clone();

    let rows = removals();
    let mut created: Vec<(String, &Removal)> = Vec::new();
    for row in &rows {
        let mut body = row.body.clone();
        if row.satellite {
            body["satelliteId"] = json!(satellite_id);
        }
        created.push((create_ok(&router, row.case, body).await, row));
    }
    // One wait for the whole table: the validation span is the same for every row, so the
    // rows that assert a settled status are all due after it, and the rows that assert
    // `VALIDATING` are deleted BEFORE it below.
    for (id, row) in &created {
        if !row.settled {
            delete_and_assert_gone(&router, row.case, id, row.status).await;
        }
    }
    wait_out_validation();
    for (id, row) in &created {
        if row.settled {
            delete_and_assert_gone(&router, row.case, id, row.status).await;
        }
    }

    let left = listed_ids(&router, "every row deleted").await;
    assert!(
        left.is_empty(),
        "case every row deleted: the store must hold no ephemeris, got {left:?}"
    );
}

/// Delete the ephemeris `id` names, having first asserted it is in `status`, and then
/// assert it is gone from every reader: the read of it, the listing, the ARN that resolved
/// it and a repeat of the delete.
async fn delete_and_assert_gone(
    router: &MultiServiceRouter,
    case: &str,
    id: &str,
    status: &str,
) {
    let before = described_status(router, case, id).await;
    assert_eq!(
        before, status,
        "case {case}: expected the ephemeris to be deleted while {status}, got {before:?}"
    );
    assert!(
        listed_ids(router, case).await.contains(&id.to_string()),
        "case {case}: the ephemeris must be listed before it is deleted"
    );

    let deleted = delete(router, id).await;
    assert_eq!(
        deleted.status, 200,
        "case {case}: expected DeleteEphemeris 200, got {} {}",
        deleted.status, deleted.body
    );
    assert_eq!(
        deleted.string("ephemerisId"),
        id,
        "case {case}: DeleteEphemeris must answer the ephemeris it removed, got {}",
        deleted.body
    );

    let read = describe(router, id).await;
    assert_eq!(
        read.status, 434,
        "case {case}: a deleted ephemeris must read 434, got {} {}",
        read.status, read.body
    );
    assert_eq!(
        read.error_code(),
        "ResourceNotFoundException",
        "case {case}: wrong error shape for a deleted ephemeris, got {}",
        read.body
    );
    assert_eq!(
        read.string("message"),
        EPHEMERIS_NOT_FOUND,
        "case {case}: wrong not-found text, got {}",
        read.body
    );

    assert!(
        !listed_ids(router, case).await.contains(&id.to_string()),
        "case {case}: a deleted ephemeris must not be listed"
    );

    let tags = list_tags(router, &ephemeris_arn(id)).await;
    assert_eq!(
        tags.status, 434,
        "case {case}: the ARN of a deleted ephemeris must resolve to nothing, got {} {}",
        tags.status, tags.body
    );

    let again = delete(router, id).await;
    assert_eq!(
        again.status, 434,
        "case {case}: a repeated DeleteEphemeris must answer 434, got {} {}",
        again.status, again.body
    );
    assert_eq!(
        again.string("message"),
        EPHEMERIS_NOT_FOUND,
        "case {case}: wrong not-found text on the repeat, got {}",
        again.body
    );
}

#[tokio::test]
async fn an_ephemeris_in_force_is_held_by_every_non_terminal_contact_of_its_satellite() {
    let (router, engine) = harness_with_engine();
    // One satellite per row, each with its own ephemeris and its own contact, so a row's
    // refusal names exactly the contact of that row and one row's delete cannot be what
    // the next row observes.
    let setting: String = (0..CONTACT_STATUSES.len())
        .map(|row| (25_000 + row).to_string())
        .collect::<Vec<String>>()
        .join(",");
    let satellites = onboard(&engine, &setting);
    assert_eq!(
        satellites.len(),
        CONTACT_STATUSES.len(),
        "the onboarding must store one satellite per row, got {satellites:?}"
    );
    let profile_arn = create_mission_profile(&router, "ephemeris in use", 60, None, None).await;

    // The terminal set this service applies must be part of the enum the model declares,
    // or one of the two has moved on without the other.
    for status in TERMINAL_CONTACT_STATUSES {
        assert!(
            CONTACT_STATUSES.contains(status),
            "case {status}: a terminal status must be a modelled ContactStatus"
        );
    }

    // ONE wait for the whole table, after every row is stored.
    let mut rows: Vec<(&str, String, String)> = Vec::new();
    for (status, satellite_id) in CONTACT_STATUSES.iter().zip(&satellites) {
        let ephemeris_id = create_ok(
            &router,
            status,
            json!({
                "name": format!("held by {status}"),
                "enabled": true,
                "expirationTime": FUTURE,
                "satelliteId": satellite_id,
                "ephemeris": tle_inline(&[(1_800_000_000, 1_800_086_400)]),
            }),
        )
        .await;
        let satellite = satellite_arn(&engine, satellite_id);
        let contact_id = reserve_against(&router, status, &profile_arn, &satellite).await;
        rows.push((status, ephemeris_id, contact_id));
    }
    wait_out_validation();
    std::thread::sleep(AFTER_SETTLE);

    for (status, ephemeris_id, contact_id) in &rows {
        put_in_status(&engine, contact_id, status);
        let in_force = described_status(&router, status, ephemeris_id).await;
        assert_eq!(
            in_force, EPHEMERIS_ENABLED,
            "case {status}: the row's ephemeris must be in force, got {in_force:?}"
        );

        let deleted = delete(&router, ephemeris_id).await;

        if TERMINAL_CONTACT_STATUSES.contains(status) {
            assert_eq!(
                deleted.status, 200,
                "case {status}: a terminal contact holds nothing, so the delete is \
                 answered, got {} {}",
                deleted.status, deleted.body
            );
            let read = describe(&router, ephemeris_id).await;
            assert_eq!(
                read.status, 434,
                "case {status}: the deleted ephemeris must be gone, got {} {}",
                read.status, read.body
            );
            continue;
        }

        assert_eq!(
            deleted.status, 409,
            "case {status}: expected the modelled ResourceInUseException status 409, got \
             {} {}",
            deleted.status, deleted.body
        );
        assert_eq!(
            deleted.error_code(),
            "ResourceInUseException",
            "case {status}: wrong error shape, got {}",
            deleted.body
        );
        assert_eq!(
            deleted.string("message"),
            ephemeris_in_use_message(std::slice::from_ref(contact_id)),
            "case {status}: the refusal must name the contact that holds the ephemeris, \
             got {}",
            deleted.body
        );

        // A refused delete writes nothing: the ephemeris is still there, still in force,
        // and still listed.
        let still = described_status(&router, status, ephemeris_id).await;
        assert_eq!(
            still, EPHEMERIS_ENABLED,
            "case {status}: a refused delete leaves the ephemeris untouched, got {still:?}"
        );
        assert!(
            listed_ids(&router, status)
                .await
                .contains(&ephemeris_id.to_string()),
            "case {status}: a refused delete leaves the ephemeris listed"
        );
    }
}

/// One row of the boundary table: an ephemeris a standing non-terminal contact does NOT
/// depend on, and why it does not.
struct NotHeld {
    case: &'static str,
    /// The members the create carries beside the data, as a JSON object.
    body: Value,
    /// Whether the row names the satellite the contact is flown against, the OTHER
    /// on-boarded satellite, or no satellite at all.
    satellite: Option<usize>,
    status: &'static str,
}

#[tokio::test]
async fn a_contact_holds_only_the_ephemeris_in_force_for_its_own_satellite() {
    let (router, engine) = harness_with_engine();
    // Two satellites: the one the contact is flown against, and another one of the same
    // account whose ephemerides nothing holds.
    let satellites = onboard(&engine, "25994=Ohio 1,27424");
    assert_eq!(
        satellites.len(),
        2,
        "the onboarding must store two satellites, got {satellites:?}"
    );
    let flown = satellite_arn(&engine, &satellites[0]);
    let profile_arn = create_mission_profile(&router, "ephemeris boundary", 60, None, None).await;
    let contact_id = reserve_against(&router, "boundary", &profile_arn, &flown).await;

    let rows = vec![
        NotHeld {
            case: "in force for another satellite",
            body: json!({
                "name": "another satellite",
                "enabled": true,
                "expirationTime": FUTURE,
                "ephemeris": tle_inline(&[(1_800_000_000, 1_800_086_400)]),
            }),
            satellite: Some(1),
            status: EPHEMERIS_ENABLED,
        },
        NotHeld {
            case: "in force for no satellite",
            body: json!({
                "name": "no satellite",
                "enabled": true,
                "expirationTime": FUTURE,
                "ephemeris": tle_inline(&[(1_800_000_000, 1_800_086_400)]),
            }),
            satellite: None,
            status: EPHEMERIS_ENABLED,
        },
        NotHeld {
            case: "the flown satellite's, but disabled",
            body: json!({
                "name": "disabled ephemeris",
                "enabled": false,
                "expirationTime": FUTURE,
                "ephemeris": tle_inline(&[(1_800_000_000, 1_800_086_400)]),
            }),
            satellite: Some(0),
            status: EPHEMERIS_DISABLED,
        },
        NotHeld {
            case: "the flown satellite's, but invalid",
            body: json!({
                "name": "invalid ephemeris",
                "enabled": true,
                "expirationTime": FUTURE,
                "ephemeris": tle_inline(&[(1_800_086_400, 1_800_000_000)]),
            }),
            satellite: Some(0),
            status: EPHEMERIS_INVALID,
        },
        NotHeld {
            case: "the flown satellite's, but expired",
            body: json!({
                "name": "expired ephemeris",
                "enabled": true,
                "expirationTime": PAST,
                "ephemeris": tle_inline(&[(1_800_000_000, 1_800_086_400)]),
            }),
            satellite: Some(0),
            status: EPHEMERIS_EXPIRED,
        },
    ];

    let mut created: Vec<(&NotHeld, String)> = Vec::new();
    for row in &rows {
        let mut body = row.body.clone();
        if let Some(index) = row.satellite {
            body["satelliteId"] = json!(satellites[index]);
        }
        created.push((row, create_ok(&router, row.case, body).await));
    }
    // The contact is `SCHEDULED` and the ephemerides have settled by the time the deletes
    // run, so each row is judged against the statuses that hold then.
    wait_out_validation();
    std::thread::sleep(AFTER_SETTLE);

    for (row, ephemeris_id) in &created {
        let status = described_status(&router, row.case, ephemeris_id).await;
        assert_eq!(
            status, row.status,
            "case {}: the row's ephemeris must be {}, got {status:?}",
            row.case, row.status
        );

        let deleted = delete(&router, ephemeris_id).await;
        assert_eq!(
            deleted.status, 200,
            "case {}: no contact depends on this ephemeris, so the delete is answered, \
             got {} {}",
            row.case, deleted.status, deleted.body
        );
        assert_eq!(
            deleted.string("ephemerisId"),
            *ephemeris_id,
            "case {}: DeleteEphemeris must answer the ephemeris it removed, got {}",
            row.case,
            deleted.body
        );
    }

    // The contact the rows were judged against is untouched by any of those deletes.
    let contact = send(&router, "GET", &format!("/contact/{contact_id}"), None).await;
    assert_eq!(
        contact.status, 200,
        "case boundary: the contact must still read back, got {} {}",
        contact.status, contact.body
    );
    let contact_status = contact.string("contactStatus");
    assert!(
        !TERMINAL_CONTACT_STATUSES.contains(&contact_status.as_str()),
        "case boundary: the contact must still be non-terminal, got {contact_status:?}"
    );
}

#[tokio::test]
async fn deleting_an_ephemeris_that_was_never_stored_is_refused_and_writes_nothing() {
    let (router, engine) = harness_with_engine();
    let satellites = onboard(&engine, "25994=Ohio 1");
    let kept = create_ok(
        &router,
        "kept",
        json!({
            "name": "kept ephemeris",
            "enabled": true,
            "expirationTime": FUTURE,
            "satelliteId": satellites[0],
            "ephemeris": tle_inline(&[(1_800_000_000, 1_800_086_400)]),
        }),
    )
    .await;

    // Well-formed `Uuid`s that name no ephemeris (the modelled pattern is
    // `^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`), and one value
    // outside that pattern, which this service's validator refuses before the handler.
    for absent in [
        ABSENT_UUID,
        "ffffffff-ffff-ffff-ffff-ffffffffffff",
        "0123456f-89ab-cdef-0123-456789abcdef",
    ] {
        let refused = delete(&router, absent).await;
        assert_eq!(
            refused.status, 434,
            "case {absent}: expected 434 for an id that names no ephemeris, got {} {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "ResourceNotFoundException",
            "case {absent}: wrong error shape, got {}",
            refused.body
        );
        assert_eq!(
            refused.string("message"),
            EPHEMERIS_NOT_FOUND,
            "case {absent}: wrong not-found text, got {}",
            refused.body
        );
    }

    // An id outside the modelled `Uuid` (len 36..36, lower-case hex in five groups) never
    // reaches the handler: this service's request validator answers it, and it is a bad
    // request rather than a missing resource — the same refusal the sibling read path
    // answers (`tests/all/ephemeris_description.rs`).
    let malformed: &[(&str, &str)] = &[
        ("not hexadecimal at all", "nope"),
        ("one character short", "0000000-0000-0000-0000-000000000000"),
        ("one character long", "000000000-0000-0000-0000-000000000000"),
        ("upper-case hex", "0A1B2C3D-4E5F-4A6B-8C7D-9E0F1A2B3C4D"),
    ];
    for (case, id) in malformed {
        let refused = delete(&router, id).await;
        assert_eq!(
            refused.status, 400,
            "case {case}: expected 400 for an id outside the modelled pattern, got {} {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            BAD_REQUEST_ERROR_CODE,
            "case {case}: expected {BAD_REQUEST_ERROR_CODE}, got {}",
            refused.body
        );
        assert_eq!(
            refused.string("message"),
            INVALID_REQUEST_BODY,
            "case {case}: wrong refusal text, got {}",
            refused.body
        );
    }

    // None of those refusals disturbed the one ephemeris the store holds.
    assert_eq!(
        listed_ids(&router, "refusals write nothing").await,
        vec![kept.clone()],
        "case refusals write nothing: the store must still hold exactly the kept ephemeris"
    );
}
