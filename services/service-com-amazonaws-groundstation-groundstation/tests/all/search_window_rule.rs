// SPDX-License-Identifier: CC0-1.0

//! The window every window-filtered listing of this service is asked for, over HTTP
//! through the same front door as the rest of this target's suites.
//!
//! The RULE under test: the window a listing searches runs forwards, so a request whose
//! `endTime` precedes its `startTime` describes no interval at all and is refused —
//! `InvalidParameterException` (HTTP 431) naming `endTime` — while a window naming the
//! same instant twice still describes an interval and is answered. It is ONE rule, read by
//! `ListContacts`, `ListEphemerides` and `ListGroundStationReservations` alike
//! (`state::validate_search_window`), so this suite drives every one of them over the SAME
//! table of windows and asserts not only that each answer follows the rule but that the
//! three answers are indistinguishable from one another.
//!
//! The rows differ along what the rule generalises over: the direction of the window, by
//! how much (a fraction of a second, a second, an hour, a year), and the zero-length
//! window on the boundary between the two.

use serde_json::{json, Value};
use service_com_amazonaws_groundstation_groundstation::state::{
    parse_ground_stations, service_state,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::scheduling_loop::{harness, harness_with_engine, send};

/// The instant every window below is measured from: a round epoch-seconds value in the
/// future, as the other suites of this target use.
const BASE: f64 = 1_800_000_000.0;

/// The ground station `ListGroundStationReservations` is asked about. Nothing is reserved
/// at it in this suite — an answered listing here is an empty page, which is all this
/// suite needs from it — but it IS a station the account is served by
/// ([`harness_serving_the_station`]), since a listing only reaches the window rule for an
/// id it can resolve.
const STATION: &str = "Ohio-1";

/// A router holding this service on an account served by [`STATION`], recorded where
/// `state::GROUND_STATIONS_ENV` lands it — so what this suite asserts is the window rule
/// rather than the resolution of the id the reservation listing is addressed by.
fn harness_serving_the_station() -> MultiServiceRouter {
    let (router, engine) = harness_with_engine();
    service_state(&*engine)
        .expect("this service's state store")
        .lock()
        .ground_stations = parse_ground_stations(STATION);
    router
}

/// One window-filtered listing, as this suite drives it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Listing {
    Contacts,
    Ephemerides,
    GroundStationReservations,
}

/// The listings the shared rule is read by — one for each place the refusal used to be
/// written out.
const LISTINGS: [Listing; 3] = [
    Listing::Contacts,
    Listing::Ephemerides,
    Listing::GroundStationReservations,
];

impl Listing {
    /// The operation this listing is, for a case label.
    fn operation(self) -> &'static str {
        match self {
            Self::Contacts => "ListContacts",
            Self::Ephemerides => "ListEphemerides",
            Self::GroundStationReservations => "ListGroundStationReservations",
        }
    }

    /// The response member that carries the page of an answered call.
    fn items_member(self) -> &'static str {
        match self {
            Self::Contacts => "contactList",
            Self::Ephemerides => "ephemerides",
            Self::GroundStationReservations => "reservationList",
        }
    }

    /// Whether [`Self::items_member`]'s modelled list shape admits an EMPTY page, so an
    /// answered listing that selected nothing still carries the member.
    ///
    /// `ContactList` carries no length constraint and `GroundStationReservationList` is
    /// `len 0..100`, while `EphemeridesList` is `len 1..500`
    /// (`smithy_query(query="shape", shape="ContactList,GroundStationReservationList,\
    /// EphemeridesList")`), so only the last reports an empty page by leaving the member
    /// out.
    fn empty_page_is_serialized(self) -> bool {
        match self {
            Self::Contacts | Self::GroundStationReservations => true,
            Self::Ephemerides => false,
        }
    }
}

/// What an answer to a listing over a window amounts to, reduced to what this suite
/// compares across the three listings: the status, and — on a refusal — the error shape,
/// the member it names and the text it carries.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Answer {
    status: u16,
    error_code: String,
    parameter_name: String,
    message: String,
}

/// Ask `listing` for `window`, and reduce its answer to an [`Answer`].
async fn ask(router: &MultiServiceRouter, listing: Listing, window: (f64, f64)) -> Answer {
    let (start, end) = window;
    let reply = match listing {
        Listing::Contacts => {
            send(
                router,
                "POST",
                "/contacts",
                Some(json!({
                    "statusList": ["SCHEDULED"],
                    "startTime": start,
                    "endTime": end,
                })),
            )
            .await
        }
        Listing::Ephemerides => {
            send(
                router,
                "POST",
                "/ephemerides",
                Some(json!({"startTime": start, "endTime": end})),
            )
            .await
        }
        Listing::GroundStationReservations => {
            send(
                router,
                "GET",
                &format!("/groundstation/{STATION}/reservation?startTime={start}&endTime={end}"),
                None,
            )
            .await
        }
    };
    let answer = Answer {
        status: reply.status,
        error_code: reply.error_code(),
        parameter_name: reply
            .field("parameterName")
            .as_str()
            .unwrap_or_default()
            .to_string(),
        message: reply
            .field("message")
            .as_str()
            .unwrap_or_default()
            .to_string(),
    };
    // An answered listing carries its page as a list, unless its page member's own shape
    // forbids an empty list, in which case a page that selected nothing leaves the member
    // out and a page that selected something still carries it; a refused one carries no
    // page at all. Asserted here so a 200 that answered with something other than a page
    // cannot pass as an answer below.
    if answer.status == 200 {
        let page = reply.field(listing.items_member());
        if listing.empty_page_is_serialized() {
            assert!(
                page.is_array(),
                "case {} over {window:?}: an answered listing must carry {} as a list, got {}",
                listing.operation(),
                listing.items_member(),
                reply.body
            );
        } else {
            assert!(
                page.as_array().is_none_or(|items| !items.is_empty()),
                "case {} over {window:?}: {} admits no empty list, so an answered listing \
                 either reports entries or leaves the member out, got {}",
                listing.operation(),
                listing.items_member(),
                reply.body
            );
        }
    } else {
        assert!(
            reply.field(listing.items_member()).is_null(),
            "case {} over {window:?}: a refused listing must carry no page, got {}",
            listing.operation(),
            reply.body
        );
    }
    answer
}

#[tokio::test]
async fn every_window_filtered_listing_refuses_a_window_that_does_not_run_forwards() {
    let router = harness_serving_the_station();

    // Rows differ along the direction of the window and the size of the gap, including the
    // zero-length window on the boundary: `endTime` equal to `startTime` still describes
    // an interval, so it is answered rather than refused.
    let rows: [(&str, (f64, f64), bool); 7] = [
        ("forwards-by-an-hour", (BASE, BASE + 3600.0), true),
        ("forwards-by-a-second", (BASE, BASE + 1.0), true),
        ("forwards-by-a-year", (BASE, BASE + 31_536_000.0), true),
        ("zero-length", (BASE, BASE), true),
        ("backwards-by-a-millisecond", (BASE, BASE - 0.001), false),
        ("backwards-by-a-second", (BASE, BASE - 1.0), false),
        ("backwards-by-an-hour", (BASE + 3600.0, BASE), false),
    ];

    for (case, window, runs_forwards) in rows {
        let mut answers: Vec<(Listing, Answer)> = Vec::new();
        for listing in LISTINGS {
            let answer = ask(&router, listing, window).await;
            if runs_forwards {
                assert_eq!(
                    answer.status,
                    200,
                    "case {case}/{}: a window that runs forwards must be answered, got \
                     {answer:?}",
                    listing.operation()
                );
            } else {
                assert_eq!(
                    answer.status,
                    431,
                    "case {case}/{}: a window that runs backwards must be refused, got \
                     {answer:?}",
                    listing.operation()
                );
                assert_eq!(
                    answer.error_code,
                    "InvalidParameterException",
                    "case {case}/{}: wrong error shape, got {answer:?}",
                    listing.operation()
                );
                assert_eq!(
                    answer.parameter_name,
                    "endTime",
                    "case {case}/{}: the refusal must name the member that is wrong, got \
                     {answer:?}",
                    listing.operation()
                );
                assert_eq!(
                    answer.message,
                    "endTime must not precede startTime",
                    "case {case}/{}: wrong refusal text, got {answer:?}",
                    listing.operation()
                );
            }
            answers.push((listing, answer));
        }

        // One rule, so one answer: every listing answers the same window the same way,
        // down to the text of the refusal.
        let (first_listing, first) = &answers[0];
        for (listing, answer) in &answers[1..] {
            assert_eq!(
                answer,
                first,
                "case {case}: {} and {} must answer the same window identically, got \
                 {answer:?} against {first:?}",
                listing.operation(),
                first_listing.operation()
            );
        }
    }
}

#[tokio::test]
async fn a_backwards_window_is_refused_before_any_other_filter_is_judged() {
    let router = harness();

    // The refusal is the window's own, so it does not depend on what else the request
    // names: each row adds a filter that would otherwise decide the answer — an
    // availability search missing the members it needs (`ListContacts`), a satellite this
    // account is not on-boarded to (`ListEphemerides`), a `nextToken` no call issued — and
    // every one is still refused on `endTime`.
    let absent_satellite = "00000000-0000-0000-0000-000000000000";
    let rows: [(&str, Listing, Value, &str); 3] = [
        (
            "available-without-the-pairing",
            Listing::Contacts,
            json!({"statusList": ["AVAILABLE"], "startTime": BASE, "endTime": BASE - 1.0}),
            "/contacts",
        ),
        (
            "an-unknown-satellite",
            Listing::Ephemerides,
            json!({
                "startTime": BASE,
                "endTime": BASE - 1.0,
                "satelliteId": absent_satellite,
            }),
            "/ephemerides",
        ),
        (
            "a-token-no-call-issued",
            Listing::Ephemerides,
            json!({"startTime": BASE, "endTime": BASE - 1.0}),
            "/ephemerides?nextToken=ephemerides.0",
        ),
    ];

    for (case, listing, body, route) in rows {
        let reply = send(&router, "POST", route, Some(body)).await;
        assert_eq!(
            reply.status,
            431,
            "case {case}/{}: expected 431, got {} {}",
            listing.operation(),
            reply.status,
            reply.body
        );
        assert_eq!(
            reply.string("parameterName"),
            "endTime",
            "case {case}/{}: the window is judged first, so the refusal names endTime, got {}",
            listing.operation(),
            reply.body
        );
        assert_eq!(
            reply.string("message"),
            "endTime must not precede startTime",
            "case {case}/{}: wrong refusal text, got {}",
            listing.operation(),
            reply.body
        );
    }
}
