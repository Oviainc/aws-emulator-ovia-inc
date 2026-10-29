// SPDX-License-Identifier: CC0-1.0

//! The references an `AVAILABLE` search of `ListContacts` (`POST /contacts`) computes its
//! windows FROM, end to end over HTTP through the same front door as the rest of this
//! target's suites.
//!
//! The RULE under test: a reference an operation COMPUTES from — as opposed to one it merely
//! filters on — is resolved against this account before anything is computed, so one that
//! names no resource this account holds is answered as the unresolved reference it carries
//! rather than with a page. An availability search's three required members name exactly one
//! pairing to compute offered windows for ("If statusList contains AVAILABLE, the request
//! must include groundStation, missionprofileArn, and satelliteArn" — the `ListContacts`
//! doc), so all three are resolved: the satellite and the mission profile as the missing
//! resources every operation that resolves them reports (`ResourceNotFoundException`, HTTP
//! 434), the ground station as the bad parameter every operation whose subject is a station
//! named in a member reports (`InvalidParameterException`, HTTP 431, naming the member). An
//! empty page would be indistinguishable from "this pairing has no pass in that window".
//!
//! So the suites below assert that rule over SETS of inputs rather than over one call:
//!
//! * a table over WHICH of the three references a request leaves unresolved — each in turn,
//!   and the row that resolves all three — with the refusal each carries;
//! * the satellite half over the whole catalogue of well-formed `satelliteArn`s that name no
//!   onboarding of this account (`crate::contact_satellite_reference_integrity`), crossed
//!   with two search windows — one holding a pass this pairing could fly and one holding
//!   none — so the refusal cannot be coming from the window; each row put to `ReserveContact`
//!   too, which must answer the same refusal, since the search is the step that feeds it;
//! * the ground-station half over a table of names spanning HOW the account knows a station
//!   (its own inventory, a satellite onboarding, neither) and the form of the name (carrying
//!   a space, hyphenated, at each published length bound, differing from a known one only in
//!   case, a prefix of one, shaped like one);
//! * the BOUNDARY of the rule: the same two members on a listing that does NOT ask for
//!   `AVAILABLE` are FILTERS of the contacts the account holds, which narrow the answer and
//!   never refuse it.

use serde_json::{json, Value};
use service_com_amazonaws_groundstation_groundstation::state::{
    ground_station_not_found_message, satellite_not_found_message, CONTACT_AVAILABLE,
    MISSION_PROFILE_NOT_FOUND,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::contact_satellite_reference_integrity::absent_satellite_arns;
use crate::ephemeris_catalog::{az_el_inline, create_ok, onboard, wait_out_validation, FUTURE};
use crate::ephemeris_deletion::satellite_arn;
use crate::scheduling_loop::{
    create_mission_profile, harness_with_engine, send, stock_ground_stations, Reply,
    ABSENT_MISSION_PROFILE_ARN,
};

/// The ground station this suite's satellite is on-boarded to, and the station
/// `ephemeris_catalog::az_el_inline` supplies its pointing angles for — so the pairing of
/// the two has passes to offer. A `GroundStationName` is 4..97 characters of
/// `[ a-zA-Z0-9-._:=]` (`smithy_query(query="shape", shape="GroundStationName")`), so the
/// space is part of the name AWS spells its stations with.
const GROUND_STATION: &str = "Ohio 1";

/// A station the ACCOUNT'S OWN INVENTORY records, rather than one reached through a satellite
/// onboarding: the two places a ground station this account is served by is recorded.
const INVENTORY_STATION: &str = "Punta Arenas 1";

/// The NORAD catalogue number this suite's one onboarding is recorded under, inside
/// `state::NORAD_SATELLITE_ID_RANGE`.
const NORAD: i32 = 25_994;

/// The pre- and post-pass durations the mission profile here is created with, and the
/// minimum viable contact duration it declares — below the length of the pass the stored
/// pointing data describes, so that pass is offered.
const PRE_PASS: i64 = 120;
const POST_PASS: i64 = 60;
const MINIMUM_SECONDS: i64 = 30;

/// The pass `ephemeris_catalog::az_el_inline` describes, as an offset from the segment's
/// reference epoch: five points ten seconds apart, every one of them above the horizon, so
/// the run of points above the horizon spans the first to the last.
const PASS_SECONDS: i64 = 40;

/// `now`, in epoch seconds: a window already flown offers nothing, so every window this
/// suite searches is placed relative to the clock rather than at a fixed instant.
fn now_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs() as i64
}

/// The account every suite below runs against: on-boarded to one satellite at
/// [`GROUND_STATION`], served by [`INVENTORY_STATION`] through its own inventory, holding
/// enabled pointing data that describes one pass of that satellite over that station, and
/// carrying one mission profile for the searches to be computed under.
struct Account {
    router: MultiServiceRouter,
    /// The `satelliteId` of the one on-boarded satellite.
    satellite_id: String,
    /// That satellite's ARN, which is what `ListContacts` names it by.
    satellite_arn: String,
    profile_arn: String,
    /// The one pass the stored pointing data describes.
    pass: (i64, i64),
}

impl Account {
    /// A window holding [`Self::pass`], which the pairing this account holds data for can
    /// still fly.
    fn window_with_the_pass(&self) -> (i64, i64) {
        (self.pass.0 - 3_600, self.pass.1 + 3_600)
    }

    /// A window holding no pass at all — a day after the data runs out — so a row answered
    /// over it would be answered with an empty page if the request resolved.
    fn window_without_a_pass(&self) -> (i64, i64) {
        (self.pass.1 + 86_400, self.pass.1 + 172_800)
    }
}

/// Stand that account up.
async fn account() -> Account {
    let (router, engine) = harness_with_engine();
    let onboarded = onboard(&engine, &format!("{NORAD}={GROUND_STATION}"));
    let satellite_id = onboarded
        .first()
        .cloned()
        .expect("the satellite is on-boarded");
    let satellite_arn = satellite_arn(&engine, &satellite_id);
    stock_ground_stations(&engine, INVENTORY_STATION);

    // The pointing data, placed ahead of the clock so the pass it describes is still
    // flyable, and left to finish validating before it is in force.
    let base = now_seconds() + 3_600;
    create_ok(
        &router,
        "pointing data",
        json!({
            "name": "pointing data",
            "enabled": true,
            "satelliteId": satellite_id,
            "expirationTime": FUTURE,
            "ephemeris": az_el_inline(&[(base, base + 3_600)]),
        }),
    )
    .await;
    wait_out_validation();

    let profile_arn = create_mission_profile(
        &router,
        "availability references",
        MINIMUM_SECONDS,
        Some(PRE_PASS),
        Some(POST_PASS),
    )
    .await;
    Account {
        router,
        satellite_id,
        satellite_arn,
        profile_arn,
        pass: (base, base + PASS_SECONDS),
    }
}

/// An `AVAILABLE` search over `window` for the pairing of `ground_station`, `satellite_arn`
/// and `profile_arn`.
fn availability(
    window: (i64, i64),
    ground_station: &str,
    satellite_arn: &str,
    profile_arn: &str,
) -> Value {
    json!({
        "statusList": [CONTACT_AVAILABLE],
        "startTime": window.0,
        "endTime": window.1,
        "groundStation": ground_station,
        "satelliteArn": satellite_arn,
        "missionProfileArn": profile_arn,
    })
}

/// `ListContacts` with `body`.
async fn list(router: &MultiServiceRouter, body: Value) -> Reply {
    send(router, "POST", "/contacts", Some(body)).await
}

/// The `contactList` of an answered listing.
fn page_of(reply: &Reply, case: &str) -> Vec<Value> {
    assert_eq!(
        reply.status, 200,
        "case {case}: expected ListContacts 200, got {} {}",
        reply.status, reply.body
    );
    reply
        .field("contactList")
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: contactList must be a list, got {}", reply.body))
        .clone()
}

/// The `(startTime, endTime)` of each record on a page, in the order the listing gave them.
fn windows_of(page: &[Value]) -> Vec<(i64, i64)> {
    page.iter()
        .map(|item| {
            let at = |member: &str| {
                item.get(member)
                    .and_then(Value::as_f64)
                    .map(|seconds| seconds as i64)
                    .unwrap_or(i64::MIN)
            };
            (at("startTime"), at("endTime"))
        })
        .collect()
}

/// What this service answers for a reference that resolves to nothing: the modelled status,
/// the error shape, the member at fault when the refusal names one, and the text.
#[derive(Debug, PartialEq, Eq)]
struct Refusal {
    status: u16,
    error: String,
    parameter: Option<String>,
    message: String,
}

impl Refusal {
    /// The refusal of a `satelliteArn` naming no onboarding of this account: the missing
    /// resource both writers of a contact version report for it, named by the id the ARN
    /// carries.
    fn satellite(satellite_id: &str) -> Self {
        Self {
            status: 434,
            error: "ResourceNotFoundException".to_string(),
            parameter: None,
            message: satellite_not_found_message(satellite_id),
        }
    }

    /// The refusal of a ground station this account cannot address: the bad parameter every
    /// operation whose subject is a station named in a member reports, naming that member.
    fn ground_station(member: &str, name: &str) -> Self {
        Self {
            status: 431,
            error: "InvalidParameterException".to_string(),
            parameter: Some(member.to_string()),
            message: ground_station_not_found_message(name),
        }
    }

    /// The refusal of a `missionProfileArn` naming no profile of this account.
    fn mission_profile() -> Self {
        Self {
            status: 434,
            error: "ResourceNotFoundException".to_string(),
            parameter: None,
            message: MISSION_PROFILE_NOT_FOUND.to_string(),
        }
    }

    /// `reply` read as a refusal, so two answers are compared as a whole.
    fn of(reply: &Reply) -> Self {
        Self {
            status: reply.status,
            error: reply.error_code(),
            parameter: reply
                .field("parameterName")
                .as_str()
                .map(str::to_string),
            message: reply
                .field("message")
                .as_str()
                .unwrap_or_default()
                .to_string(),
        }
    }
}

/// Which of the three references a row of the table below leaves unresolved.
#[derive(Clone, Copy, Debug)]
enum Unresolved {
    Satellite,
    GroundStation,
    MissionProfile,
    /// Nothing: the row names a pairing that resolves whole.
    Nothing,
}

#[tokio::test]
async fn every_reference_an_availability_search_computes_from_is_resolved() {
    let account = account().await;
    // A well-formed `satelliteArn` naming no onboarding of this account, and the id it
    // carries — one row of the catalogue the suite below spans in full.
    let (_, absent_satellite, absent_satellite_id) =
        absent_satellite_arns(&account.satellite_id)
            .into_iter()
            .next()
            .expect("the catalogue holds a row");
    // A `GroundStationName` nothing records for this account.
    let absent_station = "Bahrain-1";

    let rows: Vec<(&str, Unresolved, Option<Refusal>)> = vec![
        (
            "the satellite names no onboarding",
            Unresolved::Satellite,
            Some(Refusal::satellite(&absent_satellite_id)),
        ),
        (
            "the ground station is one the account cannot address",
            Unresolved::GroundStation,
            Some(Refusal::ground_station("groundStation", absent_station)),
        ),
        (
            "the mission profile names no profile",
            Unresolved::MissionProfile,
            Some(Refusal::mission_profile()),
        ),
        ("every reference resolves", Unresolved::Nothing, None),
    ];

    for (case, unresolved, expected) in rows {
        let body = availability(
            account.window_with_the_pass(),
            match unresolved {
                Unresolved::GroundStation => absent_station,
                _ => GROUND_STATION,
            },
            match unresolved {
                Unresolved::Satellite => &absent_satellite,
                _ => &account.satellite_arn,
            },
            match unresolved {
                Unresolved::MissionProfile => ABSENT_MISSION_PROFILE_ARN,
                _ => &account.profile_arn,
            },
        );
        let reply = list(&account.router, body).await;
        match expected {
            Some(refusal) => assert_eq!(
                Refusal::of(&reply),
                refusal,
                "case {case:?}: an unresolved {unresolved:?} must be refused as the reference \
                 it carries, got {} {}",
                reply.status,
                reply.body
            ),
            // The row that resolves whole is the control: the search is COMPUTED, so it
            // answers the pass the stored pointing data puts above that station's horizon —
            // which is what shows the rows above are refused for their reference and not
            // because the search refuses everything.
            None => {
                let page = page_of(&reply, case);
                assert_eq!(
                    windows_of(&page),
                    vec![account.pass],
                    "case {case:?}: the pairing's one flyable pass must be offered, got {page:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn an_availability_search_refuses_every_satellite_arn_this_account_holds_nothing_under() {
    let account = account().await;

    // Every row of the catalogue, crossed with a window that holds a pass of this pairing
    // and one that holds none: what is refused is the REFERENCE, so the window it is asked
    // about cannot change the answer.
    for (row, arn, satellite_id) in absent_satellite_arns(&account.satellite_id) {
        for (window_case, window) in [
            ("over a window holding a pass", account.window_with_the_pass()),
            ("over a window holding none", account.window_without_a_pass()),
        ] {
            let case = format!("{row}, {window_case}");
            let refused = list(
                &account.router,
                availability(window, GROUND_STATION, &arn, &account.profile_arn),
            )
            .await;
            assert_eq!(
                Refusal::of(&refused),
                Refusal::satellite(&satellite_id),
                "case {case:?}: the search must be refused as the missing satellite it names, \
                 got {} {}",
                refused.status,
                refused.body
            );

            // And the step the search feeds answers the same reference the same way: a
            // caller cannot be offered a pairing the reservation would refuse.
            let reserved = send(
                &account.router,
                "POST",
                "/contact",
                Some(json!({
                    "missionProfileArn": account.profile_arn,
                    "groundStation": GROUND_STATION,
                    "satelliteArn": arn,
                    "startTime": window.0,
                    "endTime": window.0 + 600,
                })),
            )
            .await;
            assert_eq!(
                Refusal::of(&reserved),
                Refusal::of(&refused),
                "case {case:?}: the discovery step and the reservation step must answer one \
                 refusal, got {} {}",
                reserved.status,
                reserved.body
            );
        }
    }
}

/// How the account under test comes to know the ground station a row names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Known {
    /// A satellite this account is on-boarded to is on-boarded to the station.
    Onboarding,
    /// The account's own inventory records it.
    Inventory,
    /// Neither place records it, so the account cannot address it.
    NotAtAll,
}

#[tokio::test]
async fn an_availability_search_refuses_a_ground_station_this_account_cannot_address() {
    let account = account().await;

    // The names span the two places a station this account is served by is recorded, and the
    // form a `GroundStationName` may take — 4..97 characters of `[ a-zA-Z0-9-._:=]`
    // (`smithy_query(query="shape", shape="GroundStationName")`) — at both published length
    // bounds. The unknown rows span WHY the account cannot address the name: nothing records
    // it, a station it knows in another case (a station is addressed by its exact name), a
    // prefix of one it knows, a name shaped like one it knows, and the longest name the
    // model admits.
    let rows: Vec<(&str, String, Known)> = vec![
        (
            "reached through the satellite onboarding, a name carrying a space",
            GROUND_STATION.to_string(),
            Known::Onboarding,
        ),
        (
            "recorded by the account's own inventory",
            INVENTORY_STATION.to_string(),
            Known::Inventory,
        ),
        (
            "nothing records it, hyphenated",
            "Bahrain-1".to_string(),
            Known::NotAtAll,
        ),
        (
            "a station it knows, in another case",
            GROUND_STATION.to_lowercase(),
            Known::NotAtAll,
        ),
        (
            "a prefix of a station it knows, at the shortest name the model admits",
            "Ohio".to_string(),
            Known::NotAtAll,
        ),
        (
            "a name shaped like one it knows",
            "Ohio 2".to_string(),
            Known::NotAtAll,
        ),
        (
            "the longest name the model admits, recorded nowhere",
            "Y".repeat(97),
            Known::NotAtAll,
        ),
    ];

    for (case, station, known) in rows {
        let case = format!("{case} ({station})");
        let reply = list(
            &account.router,
            availability(
                account.window_with_the_pass(),
                &station,
                &account.satellite_arn,
                &account.profile_arn,
            ),
        )
        .await;
        if known == Known::NotAtAll {
            assert_eq!(
                Refusal::of(&reply),
                Refusal::ground_station("groundStation", &station),
                "case {case:?}: the search must be refused as the bad parameter it carries, \
                 got {} {}",
                reply.status,
                reply.body
            );
            continue;
        }

        // A station the account IS served by is searched: the pointing data in force is for
        // one of them, so that row is offered its pass and the other an empty page — both
        // ANSWERED, which is what distinguishes "no pass here" from "no such station".
        let page = page_of(&reply, &case);
        let expected = if station == GROUND_STATION {
            vec![account.pass]
        } else {
            Vec::new()
        };
        assert_eq!(
            windows_of(&page),
            expected,
            "case {case:?}: a station this account is served by must be answered with the \
             passes its data describes, got {page:?}"
        );
    }
}

#[tokio::test]
async fn the_same_members_narrow_a_listing_of_held_contacts_instead_of_refusing_it() {
    let account = account().await;
    let (_, absent_satellite, _) = absent_satellite_arns(&account.satellite_id)
        .into_iter()
        .next()
        .expect("the catalogue holds a row");

    // Asking for a status a stored contact HOLDS names no pairing to compute anything from,
    // so the same two members are filters over the contacts the account holds: a filter
    // selects nothing when nothing matches it, which is an answered request carrying an
    // empty page rather than a refusal.
    let rows: Vec<(&str, Value)> = vec![
        (
            "a satelliteArn filter naming no onboarding",
            json!({"satelliteArn": absent_satellite}),
        ),
        (
            "a groundStation filter naming no station of the account",
            json!({"groundStation": "Bahrain-1"}),
        ),
        (
            "both at once",
            json!({"satelliteArn": absent_satellite, "groundStation": "Bahrain-1"}),
        ),
    ];

    for (case, filters) in rows {
        let mut body = json!({
            "statusList": ["SCHEDULING", "SCHEDULED", "CANCELLED"],
            "startTime": account.window_with_the_pass().0,
            "endTime": account.window_with_the_pass().1,
        });
        for (member, value) in filters.as_object().into_iter().flatten() {
            body[member] = value.clone();
        }
        let page = page_of(&list(&account.router, body).await, case);
        assert!(
            page.is_empty(),
            "case {case:?}: a filter nothing matches must select nothing, got {page:?}"
        );
    }
}
