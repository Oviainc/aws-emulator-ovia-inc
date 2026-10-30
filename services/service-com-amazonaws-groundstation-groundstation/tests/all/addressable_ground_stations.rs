// SPDX-License-Identifier: CC0-1.0

//! Which ground station a request may address, end to end over HTTP.
//!
//! The RULE under test: an operation whose request names a ground station in a required
//! member — the write that holds antenna time at one (`ReserveContact`) and the two
//! listings bound under `{groundStationId}` that report what is held there
//! (`ListGroundStationReservations`, `ListAntennas`) — serves only a station this account
//! is served by: one its own inventory records, or one a satellite onboarding of this
//! account reaches. A name neither place records addresses nothing, so the request is
//! refused with the modelled `InvalidParameterException` (HTTP 431) naming that member and
//! carrying this service's one not-found text, before anything is written.
//!
//! So the suites below assert that rule over SETS of inputs rather than one call:
//!
//! * a table of ground stations differing along HOW the account comes to know the station
//!   (an inventory entry with antennas, one with no antenna, a satellite's onboarding, and
//!   neither) and along the NAME's form (carrying a space, hyphenated, spelled over the
//!   whole character set the model admits, at each published length bound, differing from
//!   a known one only in case, a prefix of one, shaped like one) — each name put to all
//!   three operations, which must agree on it;
//! * the accepted rows round-trip: the reservation reads back through `DescribeContact` at
//!   the station it was taken for and is reported by that station's reservation listing on
//!   an antenna that station has;
//! * the refused rows write nothing: no `contactId` is minted and the contact store is as
//!   it was;
//! * a name outside the modelled shape of a `GroundStationName`, which never reaches the
//!   handler and is refused as the bad request it is.
//!
//! Every request goes through the real front door — the router, deserialization,
//! constraint validation, the handler and serialization — on the harness
//! `scheduling_loop.rs` stands up.

use serde_json::{json, Value};
use service_com_amazonaws_groundstation_groundstation::state::{
    antenna_name, BAD_REQUEST_ERROR_CODE, GROUND_STATION_INVALID,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::ephemeris_catalog::onboard;
use crate::scheduling_loop::{
    create_mission_profile, harness_with_engine, send, stock_ground_stations, Reply,
};

/// The window every reservation of this suite books: a round epoch-seconds value far
/// enough ahead that no row is refused for antenna time that has already elapsed.
const WINDOW: (i64, i64) = (1_900_000_000, 1_900_000_600);

/// The status this service's bad-parameter refusal carries
/// (`smithy_query(query="operation", operation="ReserveContact")` —
/// `InvalidParameterException`, HTTP 431), and the shape that carries it.
const REFUSED_STATUS: u16 = 431;

/// See [`REFUSED_STATUS`].
const REFUSED_ERROR: &str = "InvalidParameterException";

/// The antennas an inventory entry of this suite records at a station: two, so an accepted
/// reservation is placed on one the station actually has rather than on the single antenna
/// this emulator numbers for a station it was told nothing about.
const RECORDED_ANTENNAS: [&str; 2] = ["Antenna 1", "Antenna 2"];

/// The NORAD catalogue number this suite's one onboarding is recorded under, inside
/// `state::NORAD_SATELLITE_ID_RANGE`.
const NORAD: i32 = 25_994;

/// How the account under test comes to know the ground station a row names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Known {
    /// Its own inventory records the station, with [`RECORDED_ANTENNAS`] at it.
    Inventory,
    /// Its own inventory records the station and names no antenna at it.
    InventoryWithNoAntenna,
    /// The inventory records nothing: the station is reached because a satellite this
    /// account is on-boarded to is on-boarded to it.
    Onboarding,
    /// Neither place records it, so the account cannot address it.
    NotAtAll,
}

impl Known {
    /// Whether a request naming this row's station is served.
    fn addressable(self) -> bool {
        self != Known::NotAtAll
    }

    /// The antennas a request may be told are at this row's station.
    fn antennas(self) -> Vec<String> {
        match self {
            Known::Inventory => RECORDED_ANTENNAS.iter().map(|&name| name.into()).collect(),
            _ => Vec::new(),
        }
    }
}

/// One row: a ground station name, and how the account comes to know it.
struct Row {
    case: &'static str,
    station: String,
    known: Known,
}

/// The rows, each on a store of its own.
///
/// The names span what a `GroundStationName` may be — 4..97 characters of
/// `[ a-zA-Z0-9-._:=]` (`smithy_query(query="shape", shape="GroundStationName")`) — at both
/// published length bounds and over the character set, and the unknown rows span WHY the
/// account cannot address the name: nothing records it, a station it knows in another case
/// (a station is addressed by its exact name), a prefix of one it knows, a name shaped like
/// one it knows, and a name at the longest length nothing records.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "recorded with antennas, a name carrying a space",
            station: "Ohio 1".to_string(),
            known: Known::Inventory,
        },
        Row {
            case: "recorded with no antenna, hyphenated",
            station: "Dubbo-1".to_string(),
            known: Known::InventoryWithNoAntenna,
        },
        Row {
            case: "reached only through a satellite onboarding",
            station: "Punta Arenas 1".to_string(),
            known: Known::Onboarding,
        },
        Row {
            case: "recorded at the shortest name the model admits",
            station: "Ohio".to_string(),
            known: Known::Inventory,
        },
        Row {
            case: "recorded at the longest name the model admits",
            station: "Z".repeat(97),
            known: Known::Inventory,
        },
        Row {
            case: "recorded, spelled over the character set the model admits",
            station: "gs-1.a_b:c 2".to_string(),
            known: Known::Inventory,
        },
        Row {
            case: "nothing records it",
            station: "Bahrain-1".to_string(),
            known: Known::NotAtAll,
        },
        Row {
            case: "a station it knows, in another case",
            station: "ohio 1".to_string(),
            known: Known::NotAtAll,
        },
        Row {
            case: "a prefix of a station it knows",
            station: "Dubb".to_string(),
            known: Known::NotAtAll,
        },
        Row {
            case: "a name shaped like one it knows",
            station: "Ohio 2".to_string(),
            known: Known::NotAtAll,
        },
        Row {
            case: "a name at the longest length nothing records",
            station: "Y".repeat(97),
            known: Known::NotAtAll,
        },
    ]
}

/// `ground_station` as ONE path segment: a `GroundStationName` may carry spaces, which a
/// path segment cannot hold raw.
fn segment(ground_station: &str) -> String {
    ground_station.replace(' ', "%20")
}

/// `ReserveContact` for [`WINDOW`] at `ground_station`, answering the reply as it came.
async fn reserve(
    router: &MultiServiceRouter,
    profile_arn: &str,
    ground_station: &str,
) -> Reply {
    send(
        router,
        "POST",
        "/contact",
        Some(json!({
            "missionProfileArn": profile_arn,
            "groundStation": ground_station,
            "startTime": WINDOW.0,
            "endTime": WINDOW.1,
        })),
    )
    .await
}

/// `ListGroundStationReservations` for `ground_station` over [`WINDOW`].
async fn held_at(router: &MultiServiceRouter, ground_station: &str) -> Reply {
    send(
        router,
        "GET",
        &format!(
            "/groundstation/{}/reservation?startTime={}&endTime={}",
            segment(ground_station),
            WINDOW.0,
            WINDOW.1
        ),
        None,
    )
    .await
}

/// `ListAntennas` for `ground_station`.
async fn antennas_at(router: &MultiServiceRouter, ground_station: &str) -> Reply {
    send(
        router,
        "GET",
        &format!("/groundstation/{}/antenna", segment(ground_station)),
        None,
    )
    .await
}

/// Every `contactId` the contact store holds, as `ListContacts` reports them over a window
/// containing [`WINDOW`] — so "a refused reservation wrote nothing" is observed through the
/// front door.
async fn stored_contacts(router: &MultiServiceRouter) -> Vec<String> {
    let reply = send(
        router,
        "POST",
        "/contacts",
        Some(json!({
            "statusList": ["SCHEDULING", "SCHEDULED", "CANCELLED", "FAILED_TO_SCHEDULE"],
            "startTime": WINDOW.0 - 86_400,
            "endTime": WINDOW.1 + 86_400,
            "maxResults": 100,
        })),
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "ListContacts expected 200, got {} {}",
        reply.status, reply.body
    );
    reply
        .field("contactList")
        .as_array()
        .unwrap_or_else(|| panic!("contactList must be a list, got {}", reply.body))
        .iter()
        .map(|item| {
            item.get("contactId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// Assert `reply` is this service's refusal of a ground station the account cannot address:
/// the modelled status and shape, the member `parameter` at fault, and the one fixed text
/// this service answers for a name it cannot resolve — which echoes no part of the request
/// back, so the `ground_station` the row asked for must not appear in it.
fn assert_refused(reply: &Reply, case: &str, parameter: &str, ground_station: &str) {
    assert_eq!(
        reply.status, REFUSED_STATUS,
        "case {case:?}: expected the modelled {REFUSED_ERROR} status {REFUSED_STATUS}, got \
         {} {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_code(),
        REFUSED_ERROR,
        "case {case:?}: wrong error shape, got {}",
        reply.body
    );
    assert_eq!(
        reply.string("parameterName"),
        parameter,
        "case {case:?}: the refusal must name the parameter at fault, got {}",
        reply.body
    );
    assert_eq!(
        reply.string("message"),
        GROUND_STATION_INVALID,
        "case {case:?}: the refusal must carry this service's fixed text for a name it cannot \
         resolve, got {}",
        reply.body
    );
    assert!(
        !reply.string("message").contains(ground_station),
        "case {case:?}: the fixed text echoes no part of the request back, got {}",
        reply.body
    );
}

#[tokio::test]
async fn a_ground_station_is_addressable_exactly_when_this_account_is_served_by_it() {
    for row in rows() {
        let case = format!("{} ({})", row.case, row.station);
        // A store of its own per row, so what one row's account knows is not what the next
        // sees.
        let (router, engine) = harness_with_engine();
        match row.known {
            Known::Inventory => stock_ground_stations(
                &engine,
                &format!("{}={}", row.station, RECORDED_ANTENNAS.join("+")),
            ),
            Known::InventoryWithNoAntenna => stock_ground_stations(&engine, &row.station),
            Known::Onboarding => {
                let onboarded = onboard(&engine, &format!("{NORAD}={}", row.station));
                assert_eq!(
                    onboarded.len(),
                    1,
                    "case {case:?}: the onboarding must store one satellite, got {onboarded:?}"
                );
            }
            Known::NotAtAll => {}
        }
        let profile_arn = create_mission_profile(&router, "addressable", 60, None, None).await;

        let reserved = reserve(&router, &profile_arn, &row.station).await;
        let listed_reservations = held_at(&router, &row.station).await;
        let listed_antennas = antennas_at(&router, &row.station).await;

        // The three operations answer the same name the same way: a station one of them
        // serves is a station all of them serve, and a station one refuses is refused by
        // all — on the same status, the same error shape and the same text, each naming its
        // own member.
        assert_eq!(
            (
                reserved.status,
                listed_reservations.status,
                listed_antennas.status
            ),
            (
                if row.known.addressable() {
                    200
                } else {
                    REFUSED_STATUS
                },
                if row.known.addressable() {
                    200
                } else {
                    REFUSED_STATUS
                },
                if row.known.addressable() {
                    200
                } else {
                    REFUSED_STATUS
                },
            ),
            "case {case:?}: the reservation and the two listings must agree on the name, got \
             {} {}, {} {}, {} {}",
            reserved.status,
            reserved.body,
            listed_reservations.status,
            listed_reservations.body,
            listed_antennas.status,
            listed_antennas.body
        );

        if !row.known.addressable() {
            assert_refused(&reserved, &case, "groundStation", &row.station);
            assert_refused(
                &listed_reservations,
                &case,
                "groundStationId",
                &row.station,
            );
            assert_refused(&listed_antennas, &case, "groundStationId", &row.station);
            assert!(
                reserved.body.get("contactId").is_none(),
                "case {case:?}: a refused reservation mints no contactId, got {}",
                reserved.body
            );
            assert_eq!(
                stored_contacts(&router).await,
                Vec::<String>::new(),
                "case {case:?}: a refused reservation stores no contact"
            );
            continue;
        }

        // The accepted row round-trips: the contact reads back at the station it was
        // reserved at, and that station's reservation listing reports it on an antenna the
        // station has — the antennas the inventory records, or the one this emulator
        // numbers for a station it was told none for.
        let contact_id = reserved.string("contactId");
        assert_eq!(
            stored_contacts(&router).await,
            vec![contact_id.clone()],
            "case {case:?}: the accepted reservation is the one contact the store holds"
        );
        let described = send(&router, "GET", &format!("/contact/{contact_id}"), None).await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: DescribeContact expected 200, got {} {}",
            described.status, described.body
        );
        assert_eq!(
            described.string("groundStation"),
            row.station,
            "case {case:?}: the contact must read back at the station it was reserved at, got \
             {}",
            described.body
        );

        let reported = listed_reservations
            .field("reservationList")
            .as_array()
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: reservationList must be a list, got {}",
                    listed_reservations.body
                )
            })
            .clone();
        assert_eq!(
            reported.len(),
            1,
            "case {case:?}: the station must report the one reservation held at it, got \
             {reported:?}"
        );
        let held = &reported[0];
        assert_eq!(
            held.get("groundStationId").and_then(Value::as_str),
            Some(row.station.as_str()),
            "case {case:?}: the reservation is reported at the station it is held at, got \
             {held}"
        );
        let placed_on = held
            .get("antennaName")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let recorded = row.known.antennas();
        let placeable = if recorded.is_empty() {
            vec![antenna_name(1)]
        } else {
            recorded
        };
        assert!(
            placeable.contains(&placed_on),
            "case {case:?}: the pass must be placed on an antenna the station has ({placeable:?}), \
             got {placed_on:?}"
        );

        // And the antennas the station reports are the ones it was recorded with: an
        // account that was told none is told none, not the antenna the placement numbered.
        let listed: Vec<String> = listed_antennas
            .field("antennaList")
            .as_array()
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: antennaList must be a list, got {}",
                    listed_antennas.body
                )
            })
            .iter()
            .map(|item| {
                item.get("antennaName")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        assert_eq!(
            listed,
            row.known.antennas(),
            "case {case:?}: the station must report exactly the antennas it was recorded \
             with, got {}",
            listed_antennas.body
        );
    }
}

#[tokio::test]
async fn a_ground_station_outside_the_modelled_shape_never_reaches_the_handler() {
    let (router, engine) = harness_with_engine();
    stock_ground_stations(&engine, "Ohio 1=Antenna 1");
    let profile_arn = create_mission_profile(&router, "modelled shape", 60, None, None).await;

    // One step past each published bound of `GroundStationName` (4..97 characters of
    // `[ a-zA-Z0-9-._:=]` — `smithy_query(query="shape", shape="GroundStationName")`), and
    // a name carrying a character the pattern does not admit: none of them can name a
    // ground station at all, so each is refused as a bad request rather than as a station
    // this account cannot address. Both bounds themselves are rows of the table above.
    for (case, ground_station) in [
        ("one character too short", "Ohi".to_string()),
        ("one character too long", "Z".repeat(98)),
        ("a character outside the pattern", "Ohio#1".to_string()),
    ] {
        let reply = reserve(&router, &profile_arn, &ground_station).await;
        assert_eq!(
            reply.status, 400,
            "case {case}: a name outside the modelled shape is refused as a bad request, got \
             {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            BAD_REQUEST_ERROR_CODE,
            "case {case}: wrong error code, got {}",
            reply.body
        );
        assert!(
            reply.body.get("contactId").is_none(),
            "case {case}: a refused reservation mints no contactId, got {}",
            reply.body
        );
    }

    // The station the account IS served by is still reservable, so the rows above are
    // refused for their names and not because the account was left unable to reserve.
    let accepted = reserve(&router, &profile_arn, "Ohio 1").await;
    assert_eq!(
        accepted.status, 200,
        "case the-station-it-is-served-by: expected ReserveContact 200, got {} {}",
        accepted.status, accepted.body
    );
}
