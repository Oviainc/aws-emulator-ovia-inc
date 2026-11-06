// SPDX-License-Identifier: CC0-1.0

//! `ListEphemerides` (`POST /ephemerides`) over the WINDOW every request names, end to end
//! over HTTP through the same front door as the rest of this target's suites.
//!
//! The RULE under test: both window members are documented as "returns ephemerides with
//! expiration times within your specified time range"
//! (`smithy_query(query="doc", shape="ListEphemeridesRequest", member="startTime")` and
//! `member="endTime"`), so a listing selects an ephemeris exactly when the expiration in
//! force for it falls inside the window, both edges included — and an ephemeris carrying
//! NO expiration time has no expiration time inside any range, so no window selects it,
//! however wide. It is still a stored resource: `DescribeEphemeris` reports it, and it
//! never reaches `EXPIRED`, because it has no expiration for the clock to pass.
//!
//! So this suite asserts that rule over SETS of inputs rather than over one call:
//!
//! * a table of ephemerides spanning what the rule generalises over — whether the request
//!   names an `expirationTime` at all, which `EphemerisData` member carries the data
//!   (`tle`, `oem`, `azEl`), whether that data declares a validity of its own (inline) or
//!   declares none here (supplied from an Amazon S3 object, and an OEM body this emulator
//!   stores verbatim), how many validity ranges it declares, and whether the expiration in
//!   force has already passed;
//! * crossed with a table of windows — two seconds at the start of the epoch, a window a
//!   century away, the whole span between them, and, around every expiration the table
//!   holds, the window of that one instant, the instant on either side of it and an hour
//!   on either side of it — each asked for unpaged and at the smallest page size the model
//!   allows;
//! * each row's expectation COMPUTED from the rule rather than written out, so a row and a
//!   window the rule says do not meet are asserted not to.
//!
//! The harness, the signed `restJson1` requests and the ephemeris request builders are
//! [`crate::scheduling_loop`]'s and [`crate::ephemeris_catalog`]'s, reused rather than
//! rebuilt.

use serde_json::{json, Value};
use service_com_amazonaws_groundstation_groundstation::state::EPHEMERIS_EXPIRED;
use smithy_server_runtime::MultiServiceRouter;

use crate::ephemeris_catalog::{
    az_el_inline, create_ok, filters, list_all, list_ids, s3_object, tle_inline,
    wait_out_validation, OEM_BODY,
};
use crate::scheduling_loop::{harness, send};

/// The instant the expirations below are measured from: a round epoch-seconds value in the
/// future (2033-05-18), as the other suites of this target use.
const BASE: i64 = 2_000_000_000;

/// An expiration the clock has already passed (2001-09-09), so the table spans an
/// expiration behind the clock as well as ones still ahead.
const PASSED: i64 = 1_000_000_000;

/// 2100-01-01, the far end of the widest window this suite asks for.
const CENTURY: i64 = 4_102_444_800;

/// One row of the ephemeris table: the data the create supplies, the `expirationTime` it
/// names, and the expiration in force for the stored ephemeris — the one the request
/// named, or the end of the data's own validity when it named none, and `None` where
/// neither is known.
struct Row {
    case: &'static str,
    data: Value,
    expiration_time: Option<i64>,
    in_force: Option<i64>,
}

/// The table: one row per way the expiration a window is tested against is established,
/// crossed with the data forms that declare a validity of their own and the ones that
/// declare none here.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "an expirationTime named over an oem body",
            data: json!({"oem": {"oemData": OEM_BODY}}),
            expiration_time: Some(BASE),
            in_force: Some(BASE),
        },
        Row {
            case: "no expirationTime, one tle validity range",
            data: tle_inline(&[(BASE - 86_400, BASE + 3_600)]),
            expiration_time: None,
            in_force: Some(BASE + 3_600),
        },
        Row {
            case: "no expirationTime, azEl segments",
            data: az_el_inline(&[(BASE, BASE + 7_200)]),
            expiration_time: None,
            in_force: Some(BASE + 7_200),
        },
        Row {
            case: "no expirationTime, the last of two tle validity ranges",
            data: tle_inline(&[(BASE, BASE + 3_600), (BASE + 7_200, BASE + 10_800)]),
            expiration_time: None,
            in_force: Some(BASE + 10_800),
        },
        Row {
            case: "an expirationTime already passed, over a tle from an Amazon S3 object",
            data: json!({"tle": {"s3Object": s3_object("tle/aqua.txt")}}),
            expiration_time: Some(PASSED),
            in_force: Some(PASSED),
        },
        Row {
            case: "no expirationTime, an oem body, which declares no validity here",
            data: json!({"oem": {"oemData": OEM_BODY}}),
            expiration_time: None,
            in_force: None,
        },
        Row {
            case: "no expirationTime, an oem from an Amazon S3 object",
            data: json!({"oem": {"s3Object": s3_object("oem/aqua.oem")}}),
            expiration_time: None,
            in_force: None,
        },
        Row {
            case: "no expirationTime, a tle from an Amazon S3 object",
            data: json!({"tle": {"s3Object": s3_object("tle/terra.txt")}}),
            expiration_time: None,
            in_force: None,
        },
        Row {
            case: "no expirationTime, azEl segments from an Amazon S3 object",
            data: json!({"azEl": {
                "groundStation": "Ohio 1",
                "data": {"s3Object": s3_object("azel/ohio.json")},
            }}),
            expiration_time: None,
            in_force: None,
        },
    ]
}

/// The windows every row is asked for: the three that depend on nothing the table holds,
/// then, around each expiration the table does hold, the window of that one instant, the
/// instant on either side of it and an hour on either side of it.
fn windows(expirations: &[i64]) -> Vec<(String, (i64, i64))> {
    let mut asked: Vec<(String, (i64, i64))> = vec![
        ("two seconds at the start of the epoch".to_string(), (1, 2)),
        (
            "a window a century away".to_string(),
            (CENTURY, CENTURY + 1),
        ),
        ("the whole span between them".to_string(), (1, CENTURY)),
    ];
    for expiration in expirations {
        for (shape, window) in [
            ("the instant of", (*expiration, *expiration)),
            ("the instant before", (expiration - 1, expiration - 1)),
            ("the instant after", (expiration + 1, expiration + 1)),
            ("the hour up to", (expiration - 3_600, *expiration)),
            ("the hour from", (*expiration, expiration + 3_600)),
        ] {
            asked.push((format!("{shape} {expiration}"), window));
        }
    }
    asked
}

/// The `ephemerisId`s one listing reports for `window` at `page_size`, walked across as
/// many pages as it takes.
async fn listed(
    router: &MultiServiceRouter,
    window: (i64, i64),
    page_size: Option<i64>,
) -> Vec<String> {
    list_all(router, &filters(window, json!({})), page_size)
        .await
        .iter()
        .map(|item| {
            item.get("ephemerisId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

#[tokio::test]
async fn a_listing_selects_exactly_the_ephemerides_whose_expiration_falls_inside_its_window() {
    let router = harness();
    let table = rows();

    // Every row stored in ONE account, so each window is answered against the whole table
    // and a row that must not be selected has somewhere to be missing from.
    let mut stored: Vec<(String, &Row)> = Vec::new();
    for (index, row) in table.iter().enumerate() {
        let mut body = json!({
            "name": format!("window row {index}"),
            "enabled": true,
            "ephemeris": row.data,
        });
        if let Some(expiration) = row.expiration_time {
            body["expirationTime"] = json!(expiration);
        }
        stored.push((create_ok(&router, row.case, body).await, row));
    }
    wait_out_validation();

    let mut expirations: Vec<i64> = table.iter().filter_map(|row| row.in_force).collect();
    expirations.sort_unstable();
    expirations.dedup();

    for (case, window) in windows(&expirations) {
        // What the rule selects for this window: the stored ephemerides that HAVE an
        // expiration in force and whose expiration lies inside the window, both edges
        // included — in creation order, which is the order the listing reports.
        let expected: Vec<String> = stored
            .iter()
            .filter(|(_, row)| {
                row.in_force
                    .is_some_and(|at| (window.0..=window.1).contains(&at))
            })
            .map(|(id, _)| id.clone())
            .collect();

        for page_size in [None, Some(1)] {
            let reported = listed(&router, window, page_size).await;
            assert_eq!(
                reported, expected,
                "case {case} ({window:?}) maxResults={page_size:?}: expected {expected:?}, \
                 got {reported:?}"
            );
        }
    }
}

#[tokio::test]
async fn an_ephemeris_with_no_expiration_time_is_stored_and_selected_by_no_window() {
    let router = harness();

    // Only the rows the store holds no expiration for: each is created, then asked for
    // over every window this suite names, including the widest one the service's own
    // timestamps span.
    for (index, row) in rows()
        .into_iter()
        .filter(|row| row.in_force.is_none())
        .enumerate()
    {
        let id = create_ok(
            &router,
            row.case,
            json!({
                "name": format!("no expiration row {index}"),
                "enabled": true,
                "ephemeris": row.data,
            }),
        )
        .await;
        wait_out_validation();

        for (case, window) in windows(&[PASSED, BASE, CENTURY]) {
            let reported = list_ids(&router, &filters(window, json!({}))).await;
            assert!(
                reported.is_empty(),
                "case {} over {case} ({window:?}): an ephemeris carrying no expiration time \
                 has none inside the window, so no window selects it, got {reported:?}",
                row.case
            );
        }

        // It is a stored resource all the same — the window decides what a LISTING
        // selects, not what the account holds — and with no expiration to pass it never
        // reaches EXPIRED.
        let read = send(&router, "GET", &format!("/ephemeris/{id}"), None).await;
        assert_eq!(
            read.status, 200,
            "case {}: expected DescribeEphemeris 200 for the stored ephemeris, got {} {}",
            row.case, read.status, read.body
        );
        assert_eq!(
            read.string("ephemerisId"),
            id,
            "case {}: DescribeEphemeris must report the ephemeris the create minted, got {}",
            row.case,
            read.body
        );
        assert_ne!(
            read.string("status"),
            EPHEMERIS_EXPIRED,
            "case {}: an ephemeris with no expiration time has none to pass, so it must not \
             be reported as {EPHEMERIS_EXPIRED}, got {}",
            row.case,
            read.body
        );

        // Each row stands on its own: the next one is asked against an account holding
        // only itself.
        let deleted = send(&router, "DELETE", &format!("/ephemeris/{id}"), None).await;
        assert_eq!(
            deleted.status, 200,
            "case {}: expected DeleteEphemeris 200, got {} {}",
            row.case, deleted.status, deleted.body
        );
    }
}
