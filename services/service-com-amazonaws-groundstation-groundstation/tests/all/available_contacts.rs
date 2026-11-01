// SPDX-License-Identifier: CC0-1.0

//! `ListContacts` (`POST /contacts`) over the two parts of its contract that are not a scan
//! of the contact store — the `ephemeris` filter and an `AVAILABLE` search — end to end over
//! HTTP through the same front door as the rest of this target's suites.
//!
//! The RULES under test:
//!
//! * the optional `ephemeris` member is a FILTER ("filter for selecting contacts that use a
//!   specific ephemeris"), so it NARROWS the answer and never refuses the request: a filter
//!   naming an ephemeris nothing is pointed by selects nothing, which is an answered request
//!   carrying an empty page;
//! * `AVAILABLE` is a window the service OFFERS and nothing has reserved, so a search for it
//!   is COMPUTED for the satellite / ground station / mission profile the request names: the
//!   answer is the passes the pointing data in force for that pairing puts at or above the
//!   station's horizon, less the ones nothing could reserve — a pass outside the window
//!   asked about, one whose end the clock has passed, one shorter than the profile's
//!   `minimumViableContactDurationSeconds`, and one every antenna at the station is already
//!   held for — each reported with the pass's own times, the profile's pre- and post-pass
//!   periods around them, the peak elevation the data carries and the ephemeris it was read
//!   from.
//!
//! So the suites below assert those rules over SETS of inputs rather than over one call:
//!
//! * the windows offered, over a table of request windows and minimum viable durations
//!   against one ephemeris carrying a past segment, a short pass and a segment holding two
//!   passes;
//! * the peak elevation, over both `AngleUnits` the data may be measured in;
//! * the pointing data that is NOT in force, over a table spanning every reason it is not —
//!   disabled, for another station, for another satellite, for no satellite, held in an
//!   Amazon S3 object, orbital elements rather than pointing angles, never above the horizon
//!   and outside its own validity range;
//! * the priority that settles which of two overlapping ephemerides is used, in both orders;
//! * the discovery-then-reserve loop: an offered window is reserved, reads back through
//!   `DescribeContact` as the window that was offered, and stops being offered because the
//!   antenna is then held;
//! * the `ephemeris` filter, over a table of filters and the contacts and windows each
//!   selects, every one of them answered rather than refused.

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use service_com_amazonaws_groundstation_groundstation::state::{
    service_state, CONTACT_AVAILABLE, MAX_PAGE_SIZE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::ephemeris_catalog::{
    az_el_inline, create_ok, onboard, s3_object, tle_inline, wait_out_validation,
};
use crate::scheduling_loop::{
    create_mission_profile, harness_with_engine, send, Reply, ABSENT_MISSION_PROFILE_ARN,
    ABSENT_UUID, AFTER_SETTLE, REGION,
};

/// The ground station every availability search in this suite is for, and the station the
/// pointing data is supplied for. A `GroundStationName` is 4..97 characters of
/// `[ a-zA-Z0-9-._:=]` (`smithy_query(query="shape", shape="GroundStationName")`), so the
/// space is part of the name AWS spells its stations with.
pub(crate) const GROUND_STATION: &str = "Ohio 1";

/// A second ground station, so "supplied for another station" is a case this suite can put.
const OTHER_GROUND_STATION: &str = "Oregon 1";

/// The NORAD catalogue numbers this suite's two satellites are on-boarded under, inside
/// `state::NORAD_SATELLITE_ID_RANGE`.
const NORAD: (i32, i32) = (25_994, 27_424);

/// `now`, in epoch seconds: the availability rules are about what can STILL be flown, so
/// every window this suite stores is placed relative to the clock rather than at a fixed
/// instant.
pub(crate) fn now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs() as i64
}

/// One segment of azimuth-elevation pointing data: the reference epoch its points are offset
/// from, the validity range the segment declares its data covers, and its time-tagged
/// elevations as `(dt, el)`.
pub(crate) struct Segment {
    pub(crate) reference_epoch: i64,
    pub(crate) valid: (i64, i64),
    pub(crate) points: Vec<(f64, f64)>,
}

impl Segment {
    /// This segment as the request body carries it, with its angles measured in
    /// `angle_unit`. The azimuth is the same for every point: what decides a pass is the
    /// ELEVATION, which is what the suite varies — but it is spelled in the segment's own
    /// unit, since an `az` has to lie inside the range that unit documents (-180 to 360
    /// degrees, -π to 2π radians — `smithy_query(query="doc", shape="TimeAzEl",
    /// member="az")`).
    fn body(&self, angle_unit: &str) -> Value {
        let az = if angle_unit == "RADIAN" {
            AZIMUTH_DEGREES.to_radians()
        } else {
            AZIMUTH_DEGREES
        };
        let points: Vec<Value> = self
            .points
            .iter()
            .map(|(dt, el)| json!({"dt": dt, "az": az, "el": el}))
            .collect();
        json!({
            "referenceEpoch": self.reference_epoch,
            "validTimeRange": {"startTime": self.valid.0, "endTime": self.valid.1},
            "azElList": points,
        })
    }
}

/// The azimuth every point of this suite's pointing data carries, in degrees: one bearing,
/// well inside the range either unit admits, so no row's answer turns on the azimuth.
const AZIMUTH_DEGREES: f64 = 120.0;

/// An `azEl` ephemeris body: pointing angles for `ground_station`, measured in `angle_unit`
/// (an `AngleUnits` value — `smithy_query(query="shape", shape="AngleUnits")`), carrying
/// `segments`.
pub(crate) fn az_el(ground_station: &str, angle_unit: &str, segments: &[Segment]) -> Value {
    let list: Vec<Value> = segments
        .iter()
        .map(|segment| segment.body(angle_unit))
        .collect();
    json!({"azEl": {
        "groundStation": ground_station,
        "data": {"azElData": {"angleUnit": angle_unit, "azElSegmentList": list}},
    }})
}

/// A `CreateEphemeris` body for `data`, named `name`, applying to `satellite_id` when one is
/// named, asking for it to be enabled or not, at `priority` when one is named.
pub(crate) fn ephemeris_body(
    name: &str,
    satellite_id: Option<&str>,
    enabled: bool,
    priority: Option<i64>,
    data: Value,
) -> Value {
    let mut body = json!({"name": name, "enabled": enabled, "ephemeris": data});
    if let Some(satellite_id) = satellite_id {
        body["satelliteId"] = json!(satellite_id);
    }
    if let Some(priority) = priority {
        body["priority"] = json!(priority);
    }
    body
}

/// An account on-boarded to this suite's two satellites, with the router in front of it.
struct Account {
    router: MultiServiceRouter,
    /// The `satelliteId` of the satellite every search here is for.
    satellite_id: String,
    /// That satellite's ARN, which is what `ListContacts` names it by.
    satellite_arn: String,
    /// The `satelliteId` of the OTHER on-boarded satellite.
    other_satellite_id: String,
}

/// A fresh account on-boarded to two satellites at [`GROUND_STATION`].
///
/// Satellites are not created through this API, so the onboarding is seeded exactly where
/// `state::SATELLITES_ENV` lands it (`crate::ephemeris_catalog::onboard`).
fn account() -> Account {
    let (router, engine) = harness_with_engine();
    let onboarded = onboard(
        &engine,
        &format!(
            "{}={GROUND_STATION}+{OTHER_GROUND_STATION},{}={GROUND_STATION}",
            NORAD.0, NORAD.1
        ),
    );
    let [satellite_id, other_satellite_id] = <[String; 2]>::try_from(onboarded)
        .expect("two satellites are on-boarded");
    let satellite_arn = service_state(&*engine)
        .expect("this service's state store")
        .lock()
        .satellites
        .get(&satellite_id)
        .expect("the on-boarded satellite")
        .satellite_arn
        .clone();
    Account {
        router,
        satellite_id,
        satellite_arn,
        other_satellite_id,
    }
}

/// `ListContacts` with `body`.
async fn list(router: &MultiServiceRouter, body: Value) -> Reply {
    send(router, "POST", "/contacts", Some(body)).await
}

/// `ListContacts` with `body`, asserting it was ANSWERED, and the page it carries.
pub(crate) async fn list_ok(router: &MultiServiceRouter, case: &str, body: Value) -> Vec<Value> {
    let reply = list(router, body).await;
    assert_eq!(
        reply.status, 200,
        "case {case}: expected ListContacts 200, got {} {}",
        reply.status, reply.body
    );
    reply
        .field("contactList")
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: contactList must be a list, got {}",
                reply.body
            )
        })
        .clone()
}

/// Every window `ListContacts` answers for `body`, across as many pages as it takes at a
/// `maxResults` of `page_size`, in the order the listing gave them — asserting on the way
/// that no page is larger than was asked for.
async fn offered_pages(
    router: &MultiServiceRouter,
    case: &str,
    body: &Value,
    page_size: i64,
) -> Vec<Value> {
    let mut items: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    // More pages than the bounded answer could need at this page size, so a token that
    // failed to advance fails here instead of looping forever.
    for page in 0..64 {
        let mut request = body.clone();
        request["maxResults"] = json!(page_size);
        if let Some(token) = &token {
            request["nextToken"] = json!(token);
        }
        let reply = list(router, request).await;
        assert_eq!(
            reply.status, 200,
            "case {case} page-{page}: expected ListContacts 200, got {} {}",
            reply.status, reply.body
        );
        let on_page = reply
            .field("contactList")
            .as_array()
            .unwrap_or_else(|| {
                panic!(
                    "case {case} page-{page}: contactList must be a list, got {}",
                    reply.body
                )
            })
            .clone();
        assert!(
            on_page.len() as i64 <= page_size,
            "case {case} page-{page}: a page holds at most {page_size} windows, got {}",
            on_page.len()
        );
        items.extend(on_page);
        token = reply.field("nextToken").as_str().map(str::to_string);
        if token.is_none() {
            return items;
        }
    }
    panic!("case {case}: the listing of /contacts never closed");
}

/// An `AVAILABLE` search over `window` for the pairing of `satellite_arn`, [`GROUND_STATION`]
/// and `profile_arn`, with the members of `extra` added.
pub(crate) fn availability(
    window: (i64, i64),
    satellite_arn: &str,
    profile_arn: &str,
    extra: Value,
) -> Value {
    let mut body = json!({
        "statusList": [CONTACT_AVAILABLE],
        "startTime": window.0,
        "endTime": window.1,
        "groundStation": GROUND_STATION,
        "satelliteArn": satellite_arn,
        "missionProfileArn": profile_arn,
    });
    for (member, value) in extra.as_object().into_iter().flatten() {
        body[member] = value.clone();
    }
    body
}

/// A number-valued member of `item`, or `f64::NAN` when it carries none — so a missing
/// member fails the comparison it is part of rather than panicking here.
pub(crate) fn number(item: &Value, member: &str) -> f64 {
    item.get(member).and_then(Value::as_f64).unwrap_or(f64::NAN)
}

/// A string-valued member of `item`, empty when it carries none.
pub(crate) fn text(item: &Value, member: &str) -> String {
    item.get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The `(startTime, endTime)` of each offered window, in the order the listing gave them.
fn windows_of(page: &[Value]) -> Vec<(f64, f64)> {
    page.iter()
        .map(|item| (number(item, "startTime"), number(item, "endTime")))
        .collect()
}

/// The pre- and post-pass durations every mission profile in this suite is created with, so
/// an offered window's pre/post-pass period is asserted against something.
const PRE_PASS: i64 = 120;
const POST_PASS: i64 = 60;

/// The segments this suite's pointing data carries, and the passes each describes.
///
/// `base` is the instant the future data starts at. A pass is a run of points at or above the
/// horizon, so the elevations below put:
///
/// * `past`: one pass whose end the clock has already passed;
/// * `short`: one pass of 120 seconds, peaking at 45.5;
/// * `double`: TWO passes in ONE segment (the run breaks where the satellite sets), of 600
///   seconds each, peaking at 30 and at 60.
fn suite_segments(base: i64) -> Vec<Segment> {
    let now = now_seconds();
    vec![
        // past
        Segment {
            reference_epoch: now - 7_200,
            valid: (now - 7_200, now - 3_600),
            points: vec![
                (0.0, -5.0),
                (60.0, 10.0),
                (120.0, 30.0),
                (180.0, 8.0),
                (240.0, -3.0),
            ],
        },
        // short
        Segment {
            reference_epoch: base,
            valid: (base, base + 3_600),
            points: vec![
                (0.0, -5.0),
                (60.0, 10.0),
                (120.0, 45.5),
                (180.0, 8.0),
                (240.0, -3.0),
            ],
        },
        // double
        Segment {
            reference_epoch: base + 7_200,
            valid: (base + 7_200, base + 10_800),
            points: vec![
                (0.0, 5.0),
                (300.0, 30.0),
                (600.0, 2.0),
                (900.0, -10.0),
                (1_200.0, 15.0),
                (1_500.0, 60.0),
                (1_800.0, 1.0),
            ],
        },
    ]
}

/// One row of the offered-windows table: the minimum viable contact duration of the profile
/// the search is for, the window it asks about, and the passes it must be offered.
struct OfferedRow {
    case: &'static str,
    minimum_seconds: i64,
    window: (i64, i64),
    /// Each expected window as `(start, end, peak elevation)`, in start-time order.
    expected: Vec<(i64, i64, f64)>,
}

#[tokio::test]
async fn an_availability_search_offers_exactly_the_passes_the_pairing_can_still_fly() {
    let account = account();
    let base = now_seconds() + 3_600;
    let ephemeris_id = create_ok(
        &account.router,
        "store the pointing data",
        ephemeris_body(
            "ohio pointing data",
            Some(&account.satellite_id),
            true,
            None,
            az_el(GROUND_STATION, "DEGREE_ANGLE", &suite_segments(base)),
        ),
    )
    .await;
    wait_out_validation();

    let short = (base + 60, base + 180, 45.5);
    let first_of_double = (base + 7_200, base + 7_800, 30.0);
    let second_of_double = (base + 8_400, base + 9_000, 60.0);
    let rows = vec![
        OfferedRow {
            case: "every pass still ahead",
            minimum_seconds: 60,
            window: (base - 7_200, base + 86_400),
            expected: vec![short, first_of_double, second_of_double],
        },
        OfferedRow {
            case: "a minimum viable duration the short pass cannot meet",
            minimum_seconds: 300,
            window: (base - 7_200, base + 86_400),
            expected: vec![first_of_double, second_of_double],
        },
        OfferedRow {
            case: "a window holding only the short pass",
            minimum_seconds: 60,
            window: (base - 7_200, base + 200),
            expected: vec![short],
        },
        OfferedRow {
            case: "a window between the two passes of one segment",
            minimum_seconds: 60,
            window: (base + 7_900, base + 8_300),
            expected: vec![],
        },
        OfferedRow {
            case: "a minimum viable duration no pass can meet",
            minimum_seconds: 900,
            window: (base - 7_200, base + 86_400),
            expected: vec![],
        },
        OfferedRow {
            case: "a window the clock has already passed",
            minimum_seconds: 60,
            window: (base - 86_400, base - 3_700),
            expected: vec![],
        },
    ];

    for row in rows {
        let profile_arn = create_mission_profile(
            &account.router,
            &format!("offered {}", row.case),
            row.minimum_seconds,
            Some(PRE_PASS),
            Some(POST_PASS),
        )
        .await;
        let page = list_ok(
            &account.router,
            row.case,
            availability(row.window, &account.satellite_arn, &profile_arn, json!({})),
        )
        .await;

        let expected: Vec<(f64, f64)> = row
            .expected
            .iter()
            .map(|(start, end, _)| (*start as f64, *end as f64))
            .collect();
        assert_eq!(
            windows_of(&page),
            expected,
            "case {}: expected exactly those windows, in start-time order, got {page:?}",
            row.case
        );

        for (offered, (start, end, peak)) in page.iter().zip(&row.expected) {
            assert_eq!(
                text(offered, "contactStatus"),
                CONTACT_AVAILABLE,
                "case {}: an offered window is AVAILABLE, got {offered}",
                row.case
            );
            assert!(
                offered.get("contactId").is_none_or(Value::is_null),
                "case {}: nothing is reserved yet, so an offered window carries no contactId, \
                 got {offered}",
                row.case
            );
            assert!(
                offered.get("tags").is_none_or(Value::is_null)
                    && offered.get("version").is_none_or(Value::is_null),
                "case {}: nothing is reserved yet, so an offered window carries no tags and no \
                 version, got {offered}",
                row.case
            );
            assert_eq!(
                text(offered, "groundStation"),
                GROUND_STATION,
                "case {}: wrong ground station, got {offered}",
                row.case
            );
            assert_eq!(
                text(offered, "satelliteArn"),
                account.satellite_arn,
                "case {}: wrong satellite, got {offered}",
                row.case
            );
            assert_eq!(
                text(offered, "missionProfileArn"),
                profile_arn,
                "case {}: wrong mission profile, got {offered}",
                row.case
            );
            assert_eq!(
                text(offered, "region"),
                REGION,
                "case {}: wrong region, got {offered}",
                row.case
            );
            assert_eq!(
                number(offered, "prePassStartTime"),
                (*start - PRE_PASS) as f64,
                "case {}: the pre-pass period is the profile's duration ahead of the pass, got \
                 {offered}",
                row.case
            );
            assert_eq!(
                number(offered, "postPassEndTime"),
                (*end + POST_PASS) as f64,
                "case {}: the post-pass period is the profile's duration after the pass, got \
                 {offered}",
                row.case
            );
            let elevation = offered
                .get("maximumElevation")
                .unwrap_or(&Value::Null)
                .clone();
            assert_eq!(
                number(&elevation, "value"),
                *peak,
                "case {}: expected the peak the pointing data carries, got {elevation}",
                row.case
            );
            assert_eq!(
                text(&elevation, "unit"),
                "DEGREE_ANGLE",
                "case {}: expected the unit the pointing data is measured in, got {elevation}",
                row.case
            );
            let reported = offered.get("ephemeris").unwrap_or(&Value::Null).clone();
            assert_eq!(
                text(&reported, "ephemerisId"),
                ephemeris_id,
                "case {}: an offered window names the ephemeris its pointing came from, got \
                 {reported}",
                row.case
            );
            assert_eq!(
                text(&reported, "ephemerisType"),
                "AZ_EL",
                "case {}: wrong ephemerisType, got {reported}",
                row.case
            );
            assert!(
                offered
                    .get("visibilityStartTime")
                    .is_none_or(Value::is_null)
                    && offered.get("visibilityEndTime").is_none_or(Value::is_null),
                "case {}: no receive mask is recorded for a ground station, so the visibility \
                 times carry nothing, got {offered}",
                row.case
            );
        }
    }
}

#[tokio::test]
async fn an_offered_window_reports_its_peak_in_the_unit_the_pointing_data_is_measured_in() {
    // Both `AngleUnits` values (`smithy_query(query="shape", shape="AngleUnits")`), each with
    // elevations spelled in that unit: the horizon is 0 and zenith is 90 degrees or π/2
    // radians (`smithy_query(query="doc", shape="TimeAzEl", member="el")`).
    struct UnitRow {
        /// The `angleUnit` the segment declares.
        unit: &'static str,
        /// Its points, as `(dt, el)`.
        points: Vec<(f64, f64)>,
        /// The peak the offered window must report, in that unit.
        peak: f64,
    }
    let units = vec![
        UnitRow {
            unit: "DEGREE_ANGLE",
            points: vec![
                (0.0, -1.0),
                (60.0, 12.0),
                (120.0, 88.25),
                (180.0, 4.0),
                (240.0, -1.0),
            ],
            peak: 88.25,
        },
        UnitRow {
            unit: "RADIAN",
            points: vec![
                (0.0, -0.1),
                (60.0, 0.2),
                (120.0, 1.45),
                (180.0, 0.07),
                (240.0, -0.05),
            ],
            peak: 1.45,
        },
    ];

    for UnitRow { unit, points, peak } in units {
        let account = account();
        let base = now_seconds() + 3_600;
        create_ok(
            &account.router,
            unit,
            ephemeris_body(
                "pointing data",
                Some(&account.satellite_id),
                true,
                None,
                az_el(
                    GROUND_STATION,
                    unit,
                    &[Segment {
                        reference_epoch: base,
                        valid: (base, base + 3_600),
                        points,
                    }],
                ),
            ),
        )
        .await;
        wait_out_validation();
        let profile_arn =
            create_mission_profile(&account.router, unit, 60, Some(PRE_PASS), Some(POST_PASS)).await;

        let page = list_ok(
            &account.router,
            unit,
            availability(
                (base - 3_600, base + 86_400),
                &account.satellite_arn,
                &profile_arn,
                json!({}),
            ),
        )
        .await;
        assert_eq!(
            page.len(),
            1,
            "case {unit}: one pass is above the horizon, got {page:?}"
        );
        let elevation = page[0]
            .get("maximumElevation")
            .unwrap_or(&Value::Null)
            .clone();
        assert_eq!(
            (number(&elevation, "value"), text(&elevation, "unit")),
            (peak, unit.to_string()),
            "case {unit}: expected the peak in the data's own unit, got {elevation}"
        );
    }
}

/// One row of the not-in-force table: an ephemeris stored in an account of its own, which
/// offers no window for the pairing this suite searches.
struct NotInForce {
    case: &'static str,
    /// Whether the ephemeris applies to the searched satellite, the other one, or none.
    satellite: Which,
    enabled: bool,
    /// The `ephemeris` member the create carries, built against `base`.
    data: fn(i64) -> Value,
}

/// Which satellite an ephemeris of the table above applies to.
enum Which {
    Searched,
    Other,
    None,
}

#[tokio::test]
async fn pointing_data_that_is_not_in_force_for_the_pairing_offers_no_window() {
    let rows = vec![
        NotInForce {
            case: "the caller did not ask for it to be enabled",
            satellite: Which::Searched,
            enabled: false,
            data: |base| az_el(GROUND_STATION, "DEGREE_ANGLE", &visible_segment(base)),
        },
        NotInForce {
            case: "supplied for another ground station",
            satellite: Which::Searched,
            enabled: true,
            data: |base| az_el(OTHER_GROUND_STATION, "DEGREE_ANGLE", &visible_segment(base)),
        },
        NotInForce {
            case: "applies to another satellite",
            satellite: Which::Other,
            enabled: true,
            data: |base| az_el(GROUND_STATION, "DEGREE_ANGLE", &visible_segment(base)),
        },
        NotInForce {
            case: "applies to no satellite",
            satellite: Which::None,
            enabled: true,
            data: |base| az_el(GROUND_STATION, "DEGREE_ANGLE", &visible_segment(base)),
        },
        NotInForce {
            case: "its pointing data is held in an Amazon S3 object",
            satellite: Which::Searched,
            enabled: true,
            data: |_| {
                json!({"azEl": {
                    "groundStation": GROUND_STATION,
                    "data": {"s3Object": s3_object("azel/ohio-1.json")},
                }})
            },
        },
        NotInForce {
            case: "orbital elements rather than pointing angles",
            satellite: Which::Searched,
            enabled: true,
            data: |base| tle_inline(&[(base, base + 86_400)]),
        },
        NotInForce {
            case: "never at or above the horizon",
            satellite: Which::Searched,
            enabled: true,
            data: |base| {
                az_el(
                    GROUND_STATION,
                    "DEGREE_ANGLE",
                    &[Segment {
                        reference_epoch: base,
                        valid: (base, base + 3_600),
                        points: vec![
                            (0.0, -30.0),
                            (60.0, -12.0),
                            (120.0, -0.5),
                            (180.0, -9.0),
                            (240.0, -40.0),
                        ],
                    }],
                )
            },
        },
        NotInForce {
            case: "every point lies outside the validity range of its segment",
            satellite: Which::Searched,
            enabled: true,
            data: |base| {
                az_el(
                    GROUND_STATION,
                    "DEGREE_ANGLE",
                    &[Segment {
                        reference_epoch: base,
                        valid: (base, base + 30),
                        points: vec![
                            (600.0, 10.0),
                            (660.0, 20.0),
                            (720.0, 40.0),
                            (780.0, 20.0),
                            (840.0, 10.0),
                        ],
                    }],
                )
            },
        },
    ];

    for row in rows {
        let account = account();
        let base = now_seconds() + 3_600;
        let satellite_id = match row.satellite {
            Which::Searched => Some(account.satellite_id.clone()),
            Which::Other => Some(account.other_satellite_id.clone()),
            Which::None => None,
        };
        create_ok(
            &account.router,
            row.case,
            ephemeris_body(
                "pointing data",
                satellite_id.as_deref(),
                row.enabled,
                None,
                (row.data)(base),
            ),
        )
        .await;
        wait_out_validation();
        let profile_arn =
            create_mission_profile(&account.router, row.case, 60, Some(PRE_PASS), Some(POST_PASS))
                .await;

        let page = list_ok(
            &account.router,
            row.case,
            availability(
                (base - 86_400, base + 86_400),
                &account.satellite_arn,
                &profile_arn,
                json!({}),
            ),
        )
        .await;
        assert!(
            page.is_empty(),
            "case {}: no pointing data is in force for this pairing, so it must offer no \
             window, got {page:?}",
            row.case
        );
    }
}

/// One segment holding one pass of 120 seconds peaking at 45.5, starting a minute after
/// `base`.
fn visible_segment(base: i64) -> Vec<Segment> {
    vec![Segment {
        reference_epoch: base,
        valid: (base, base + 3_600),
        points: vec![
            (0.0, -5.0),
            (60.0, 10.0),
            (120.0, 45.5),
            (180.0, 8.0),
            (240.0, -3.0),
        ],
    }]
}

#[tokio::test]
async fn where_two_ephemerides_overlap_the_higher_priority_one_is_the_one_used() {
    // The same pass described twice, with different peaks, so which ephemeris was used is
    // visible in the answer. Both orders of the two priorities, so what decides it is the
    // priority rather than the order they were stored in.
    for (first_priority, second_priority) in [(1_i64, 9_i64), (9, 1)] {
        let case = format!("priorities {first_priority} then {second_priority}");
        let account = account();
        let base = now_seconds() + 3_600;
        let peaks = (45.5_f64, 61.25_f64);
        let mut stored: Vec<(i64, String, f64)> = Vec::new();
        for (priority, peak) in [(first_priority, peaks.0), (second_priority, peaks.1)] {
            let id = create_ok(
                &account.router,
                &case,
                ephemeris_body(
                    &format!("pointing data at {priority}"),
                    Some(&account.satellite_id),
                    true,
                    Some(priority),
                    az_el(
                        GROUND_STATION,
                        "DEGREE_ANGLE",
                        &[Segment {
                            reference_epoch: base,
                            valid: (base, base + 3_600),
                            points: vec![
                                (0.0, -5.0),
                                (60.0, 10.0),
                                (120.0, peak),
                                (180.0, 8.0),
                                (240.0, -3.0),
                            ],
                        }],
                    ),
                ),
            )
            .await;
            stored.push((priority, id, peak));
        }
        wait_out_validation();
        let profile_arn =
            create_mission_profile(&account.router, &case, 60, Some(PRE_PASS), Some(POST_PASS))
                .await;

        let page = list_ok(
            &account.router,
            &case,
            availability(
                (base - 3_600, base + 86_400),
                &account.satellite_arn,
                &profile_arn,
                json!({}),
            ),
        )
        .await;
        assert_eq!(
            page.len(),
            1,
            "case {case}: two ephemerides describing ONE pass offer one window, got {page:?}"
        );
        let (_, expected_id, expected_peak) = stored
            .iter()
            .max_by_key(|(priority, _, _)| *priority)
            .expect("two ephemerides were stored")
            .clone();
        let elevation = page[0]
            .get("maximumElevation")
            .unwrap_or(&Value::Null)
            .clone();
        assert_eq!(
            number(&elevation, "value"),
            expected_peak,
            "case {case}: expected the higher-priority ephemeris's peak, got {}",
            page[0]
        );
        let reported = page[0].get("ephemeris").unwrap_or(&Value::Null).clone();
        assert_eq!(
            text(&reported, "ephemerisId"),
            expected_id,
            "case {case}: expected the higher-priority ephemeris, got {reported}"
        );
    }
}

#[tokio::test]
async fn an_offered_window_can_be_reserved_and_then_stops_being_offered() {
    let account = account();
    let base = now_seconds() + 3_600;
    create_ok(
        &account.router,
        "store the pointing data",
        ephemeris_body(
            "ohio pointing data",
            Some(&account.satellite_id),
            true,
            None,
            az_el(GROUND_STATION, "DEGREE_ANGLE", &suite_segments(base)),
        ),
    )
    .await;
    wait_out_validation();
    let profile_arn = create_mission_profile(
        &account.router,
        "discovery loop",
        60,
        Some(PRE_PASS),
        Some(POST_PASS),
    )
    .await;
    let whole_window = (base - 7_200, base + 86_400);

    let offered = list_ok(
        &account.router,
        "before reserving",
        availability(
            whole_window,
            &account.satellite_arn,
            &profile_arn,
            json!({}),
        ),
    )
    .await;
    assert_eq!(
        offered.len(),
        3,
        "case before reserving: three passes are still flyable, got {offered:?}"
    );
    let taken = (
        number(&offered[0], "startTime"),
        number(&offered[0], "endTime"),
    );

    // The window the service offered is reserved as it was offered — the discovery step
    // feeding the reservation step.
    let reserved = send(
        &account.router,
        "POST",
        "/contact",
        Some(json!({
            "missionProfileArn": profile_arn,
            "groundStation": GROUND_STATION,
            "satelliteArn": account.satellite_arn,
            "startTime": taken.0,
            "endTime": taken.1,
        })),
    )
    .await;
    assert_eq!(
        reserved.status, 200,
        "case reserve-the-offered-window: a window the service offered must be reservable, got \
         {} {}",
        reserved.status, reserved.body
    );
    let contact_id = reserved.string("contactId");
    std::thread::sleep(AFTER_SETTLE);

    // Read back through DescribeContact: the reservation holds the window that was offered.
    let described = send(
        &account.router,
        "GET",
        &format!("/contact/{contact_id}"),
        None,
    )
    .await;
    assert_eq!(
        described.status, 200,
        "case describe-the-reservation: expected 200, got {} {}",
        described.status, described.body
    );
    assert_eq!(
        (
            number(&described.body, "startTime"),
            number(&described.body, "endTime"),
            number(&described.body, "prePassStartTime"),
            number(&described.body, "postPassEndTime"),
        ),
        (
            taken.0,
            taken.1,
            taken.0 - PRE_PASS as f64,
            taken.1 + POST_PASS as f64
        ),
        "case describe-the-reservation: the contact must hold the window that was offered, got {}",
        described.body
    );

    // The antenna is now held for that window, so the window is no longer on offer; the
    // others still are.
    let after = list_ok(
        &account.router,
        "after reserving",
        availability(
            whole_window,
            &account.satellite_arn,
            &profile_arn,
            json!({}),
        ),
    )
    .await;
    assert_eq!(
        windows_of(&after),
        windows_of(&offered[1..]),
        "case after reserving: a reserved window stops being offered and the rest stay, got \
         {after:?}"
    );

    // And the reservation itself is what the store now holds, under the status a reservation
    // reaches rather than under AVAILABLE.
    let scheduled = list_ok(
        &account.router,
        "the reservation itself",
        json!({
            "statusList": ["SCHEDULED", "SCHEDULING"],
            "startTime": whole_window.0,
            "endTime": whole_window.1,
        }),
    )
    .await;
    assert_eq!(
        scheduled
            .iter()
            .map(|item| text(item, "contactId"))
            .collect::<Vec<String>>(),
        vec![contact_id],
        "case the reservation itself: expected exactly the reserved contact, got {scheduled:?}"
    );
}

#[tokio::test]
async fn an_availability_search_pages_its_offered_windows_and_bounds_what_it_computes() {
    let account = account();
    let base = now_seconds() + 3_600;
    // TWO ephemerides whose passes INTERLEAVE in time while their priorities order them the
    // other way round: the high-priority one holds the even days and the low-priority one the
    // odd days, so the sequence a page offsets into cannot be a prefix of either ephemeris's
    // own passes. Each segment holds two passes of 600 seconds, so the sequence is twelve
    // windows — more than one page at the page sizes below.
    let day_segment = |day: i64| {
        let epoch = base + day * 86_400;
        Segment {
            reference_epoch: epoch,
            valid: (epoch, epoch + 3_600),
            points: vec![
                (0.0, 5.0),
                (300.0, 30.0),
                (600.0, 2.0),
                (900.0, -10.0),
                (1_200.0, 15.0),
                (1_500.0, 60.0),
                (1_800.0, 1.0),
            ],
        }
    };
    let mut by_day: Vec<(i64, String)> = Vec::new();
    for (case, priority, days) in [
        ("the high-priority ephemeris", 9_i64, [0_i64, 2, 4]),
        ("the low-priority ephemeris", 1, [1, 3, 5]),
    ] {
        let segments: Vec<Segment> = days.iter().map(|day| day_segment(*day)).collect();
        let ephemeris_id = create_ok(
            &account.router,
            case,
            ephemeris_body(
                &format!("paged pointing data at {priority}"),
                Some(&account.satellite_id),
                true,
                Some(priority),
                az_el(GROUND_STATION, "DEGREE_ANGLE", &segments),
            ),
        )
        .await;
        by_day.extend(days.iter().map(|day| (*day, ephemeris_id.clone())));
    }
    by_day.sort();
    // The whole sequence, in the start-time order a page walks it in: two windows per day,
    // each naming the ephemeris whose pointing data it was read from.
    let expected: Vec<((f64, f64), String)> = by_day
        .iter()
        .flat_map(|(day, ephemeris_id)| {
            let epoch = base + day * 86_400;
            [
                (
                    (epoch as f64, (epoch + 600) as f64),
                    ephemeris_id.clone(),
                ),
                (
                    ((epoch + 1_200) as f64, (epoch + 1_800) as f64),
                    ephemeris_id.clone(),
                ),
            ]
        })
        .collect();
    wait_out_validation();
    let profile_arn =
        create_mission_profile(&account.router, "paging", 60, Some(PRE_PASS), Some(POST_PASS)).await;

    // A window spanning decades: what it costs is bounded by constants rather than by the
    // span, and paging through it walks the sequence ONCE, in start-time order, whatever the
    // page size — so no window is offered twice and none is skipped.
    let request = availability(
        (base - 86_400, base + 2_000_000_000),
        &account.satellite_arn,
        &profile_arn,
        json!({}),
    );
    let whole = list_ok(&account.router, "a window spanning decades", request.clone()).await;
    assert!(
        whole.len() <= MAX_PAGE_SIZE,
        "case a window spanning decades: a page holds at most {MAX_PAGE_SIZE} windows, got {}",
        whole.len()
    );
    for page_size in [2_i64, 5] {
        let case = format!("paged by {page_size}");
        let paged = offered_pages(&account.router, &case, &request, page_size).await;
        let walked: Vec<((f64, f64), String)> = paged
            .iter()
            .map(|item| {
                (
                    (number(item, "startTime"), number(item, "endTime")),
                    text(
                        item.get("ephemeris").unwrap_or(&Value::Null),
                        "ephemerisId",
                    ),
                )
            })
            .collect();
        assert_eq!(
            walked, expected,
            "case {case}: paging must walk every offered window once, in start-time order and \
             under the ephemeris it came from, got {paged:?}"
        );
    }
}

#[tokio::test]
async fn an_availability_search_naming_a_mission_profile_that_does_not_exist_is_refused() {
    let account = account();
    let base = now_seconds() + 3_600;

    // The mission profile is what the pre- and post-pass periods and the minimum viable
    // duration of an offered window come from, so an ARN naming none names a missing
    // resource rather than selecting nothing: both a well-formed ARN of this account's
    // Region and one of another Region are answered the same way.
    let absent: Vec<(&str, String)> = vec![
        ("never created", ABSENT_MISSION_PROFILE_ARN.to_string()),
        (
            "another Region",
            format!(
                "arn:aws:groundstation:eu-west-1:{}:mission-profile/\
                 11111111-1111-1111-1111-111111111111",
                engine::EMULATOR_ACCOUNT_ID
            ),
        ),
    ];
    for (case, profile_arn) in absent {
        let refused = list(
            &account.router,
            availability(
                (base - 3_600, base + 86_400),
                &account.satellite_arn,
                &profile_arn,
                json!({}),
            ),
        )
        .await;
        assert_eq!(
            refused.status, 434,
            "case {case}: expected 434, got {} {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "ResourceNotFoundException",
            "case {case}: wrong error shape, got {}",
            refused.body
        );
    }

    // A profile that DOES exist is answered, so the refusal is about the profile and not
    // about the search.
    let profile_arn = create_mission_profile(
        &account.router,
        "a profile that exists",
        60,
        Some(PRE_PASS),
        Some(POST_PASS),
    )
    .await;
    let answered = list_ok(
        &account.router,
        "a profile that exists",
        availability(
            (base - 3_600, base + 86_400),
            &account.satellite_arn,
            &profile_arn,
            json!({}),
        ),
    )
    .await;
    assert!(
        answered.is_empty(),
        "case a profile that exists: no pointing data is in force, so it offers no window, got \
         {answered:?}"
    );
}

#[tokio::test]
async fn the_ephemeris_filter_narrows_the_listing_and_refuses_nothing() {
    let account = account();
    let base = now_seconds() + 3_600;
    let profile_arn = create_mission_profile(
        &account.router,
        "ephemeris filter",
        60,
        Some(PRE_PASS),
        Some(POST_PASS),
    )
    .await;

    // Three ephemerides: the one in force for the searched satellite at the searched
    // station, one in force for the OTHER satellite, and one of the searched satellite the
    // caller did not ask to enable.
    let pointing = create_ok(
        &account.router,
        "pointing data",
        ephemeris_body(
            "ohio pointing data",
            Some(&account.satellite_id),
            true,
            None,
            az_el(GROUND_STATION, "DEGREE_ANGLE", &visible_segment(base)),
        ),
    )
    .await;
    let other_satellites = create_ok(
        &account.router,
        "another satellite's data",
        ephemeris_body(
            "other satellite data",
            Some(&account.other_satellite_id),
            true,
            None,
            az_el(GROUND_STATION, "DEGREE_ANGLE", &visible_segment(base)),
        ),
    )
    .await;
    let disabled = create_ok(
        &account.router,
        "data that was not enabled",
        ephemeris_body(
            "disabled data",
            Some(&account.satellite_id),
            false,
            None,
            az_el(GROUND_STATION, "DEGREE_ANGLE", &visible_segment(base)),
        ),
    )
    .await;
    wait_out_validation();

    // One reserved contact flying the searched satellite, so the filter has a stored contact
    // to select as well as a computed window.
    let reserved = send(
        &account.router,
        "POST",
        "/contact",
        Some(json!({
            "missionProfileArn": profile_arn,
            "groundStation": GROUND_STATION,
            "satelliteArn": account.satellite_arn,
            "startTime": base + 20_000,
            "endTime": base + 20_600,
        })),
    )
    .await;
    assert_eq!(
        reserved.status, 200,
        "reserve a contact to filter: expected 200, got {} {}",
        reserved.status, reserved.body
    );
    let contact_id = reserved.string("contactId");
    std::thread::sleep(AFTER_SETTLE);

    let window = (base - 3_600, base + 86_400);
    let stored_statuses = json!(["SCHEDULED", "SCHEDULING"]);
    // Each row: the filter's `id`, the contact ids it must select from the store, and how
    // many windows an availability search under the same filter must offer.
    let rows: Vec<(&str, &str, Vec<String>, usize)> = vec![
        (
            "the ephemeris in force for the satellite the contact flies",
            &pointing,
            vec![contact_id.clone()],
            1,
        ),
        (
            "an ephemeris in force for another satellite",
            &other_satellites,
            vec![],
            0,
        ),
        (
            "an ephemeris that was never enabled",
            &disabled,
            vec![],
            0,
        ),
        ("a Uuid that names no ephemeris", ABSENT_UUID, vec![], 0),
    ];

    for (case, id, expected_contacts, expected_windows) in rows {
        let filter = json!({"azEl": {"id": id}});
        let selected = list_ok(
            &account.router,
            case,
            json!({
                "statusList": stored_statuses,
                "startTime": window.0,
                "endTime": window.1,
                "ephemeris": filter,
            }),
        )
        .await;
        assert_eq!(
            selected
                .iter()
                .map(|item| text(item, "contactId"))
                .collect::<Vec<String>>(),
            expected_contacts,
            "case {case}: the filter must select exactly the contacts the ephemeris points, got \
             {selected:?}"
        );

        let offered = list_ok(
            &account.router,
            case,
            availability(
                window,
                &account.satellite_arn,
                &profile_arn,
                json!({"ephemeris": filter}),
            ),
        )
        .await;
        assert_eq!(
            offered.len(),
            expected_windows,
            "case {case}: an offered window is selected only by a filter naming the ephemeris \
             its pointing came from, got {offered:?}"
        );
    }

    // A mixed status list answers with BOTH kinds of row: the store's contacts and the
    // computed windows.
    let mixed = list_ok(
        &account.router,
        "a mixed status list",
        json!({
            "statusList": ["SCHEDULED", "SCHEDULING", CONTACT_AVAILABLE],
            "startTime": window.0,
            "endTime": window.1,
            "groundStation": GROUND_STATION,
            "satelliteArn": account.satellite_arn,
            "missionProfileArn": profile_arn,
        }),
    )
    .await;
    assert_eq!(
        mixed
            .iter()
            .map(|item| text(item, "contactStatus"))
            .collect::<Vec<String>>(),
        vec![
            "SCHEDULED".to_string(),
            CONTACT_AVAILABLE.to_string()
        ],
        "case a mixed status list: expected the reserved contact and the offered window, got \
         {mixed:?}"
    );
}

#[tokio::test]
async fn the_az_el_helper_of_the_sibling_suite_describes_a_pass_this_search_offers() {
    // `crate::ephemeris_catalog::az_el_inline` is the az/el data the ephemeris suites store:
    // five points climbing from 20 to 24 degrees over 40 seconds, at `Ohio 1`. Every point is
    // above the horizon, so the whole span is one pass — which this search must offer, since
    // the same store answers both suites.
    let account = account();
    let base = now_seconds() + 3_600;
    create_ok(
        &account.router,
        "the sibling suite's data",
        ephemeris_body(
            "shared pointing data",
            Some(&account.satellite_id),
            true,
            None,
            az_el_inline(&[(base, base + 3_600)]),
        ),
    )
    .await;
    wait_out_validation();
    let profile_arn = create_mission_profile(
        &account.router,
        "shared data",
        30,
        Some(PRE_PASS),
        Some(POST_PASS),
    )
    .await;

    let page = list_ok(
        &account.router,
        "the sibling suite's data",
        availability(
            (base - 3_600, base + 86_400),
            &account.satellite_arn,
            &profile_arn,
            json!({}),
        ),
    )
    .await;
    assert_eq!(
        windows_of(&page),
        vec![(base as f64, (base + 40) as f64)],
        "case the sibling suite's data: the whole span above the horizon is one pass, got \
         {page:?}"
    );
}
