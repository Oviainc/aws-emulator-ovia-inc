// SPDX-License-Identifier: CC0-1.0

//! `CreateEphemeris` (`POST /ephemeris`) and `ListEphemerides` (`POST /ephemerides`), end
//! to end over HTTP through the same front door as the rest of this target's suites.
//!
//! The RULE under test: an ephemeris is a stored resource, and `ListEphemerides` REPORTS
//! the store it reads — exactly the ephemerides this account holds whose `expirationTime`
//! falls inside the window the request names and which pass its optional `satelliteId`,
//! `ephemerisType` and `statusList` filters, one `EphemerisItem` per ephemeris carrying
//! that ephemeris's own `ephemerisId`, `ephemerisType`, `status`, `priority`, `enabled`,
//! `creationTime`, `name` and `sourceS3Object`, in a stable order, a page at a time, and
//! changing nothing a later caller sees.
//!
//! So the suites below assert that rule over SETS of inputs rather than over one call:
//!
//! * a catalogue of ephemerides spanning every `EphemerisData` member a request may carry
//!   (`tle`, `oem`, `azEl`), each supplied both inline and from an Amazon S3 object, and
//!   differing in name, priority (each published bound of `CustomerEphemerisPriority`
//!   among them) and whether the caller asked for it to be enabled;
//! * the status an ephemeris earns, over a table spanning every `EphemerisStatus` this
//!   service puts an ephemeris in — `VALIDATING` straight after the create, then
//!   `ENABLED`, `DISABLED`, `INVALID` or `EXPIRED` once the validation falls due;
//! * each filter in turn, over a table of filter sets and the ids each must select,
//!   including each `EphemerisType` value, status lists of several sizes, both published
//!   edges of the window and one second past each, and a satellite the account is not
//!   on-boarded to;
//! * paging over a table of page sizes — dividing the sequence evenly, unevenly, exactly,
//!   and larger than the store — with `maxResults` asserted on each published bound of
//!   `PaginationMaxResults` and one value past each, and a `nextToken` this listing did
//!   not issue refused rather than resumed;
//! * the refusals of a create that names no one source of ephemeris data, or a satellite
//!   this account is not on-boarded to — each of which stores nothing.
//!
//! The data builders, the onboarding helper and the create-and-assert helper below are
//! `pub(crate)`, so the sibling `ephemeris_description.rs` suite stores the same
//! ephemerides through the same front door rather than standing up a second copy of them.

use std::collections::HashMap;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_groundstation_groundstation::state::{
    parse_satellites, satellite_not_found_message, service_state, ResourceRef,
    BAD_REQUEST_ERROR_CODE, DEFAULT_EPHEMERIS_PRIORITY, EPHEMERIS_DISABLED, EPHEMERIS_ENABLED,
    EPHEMERIS_EXPIRED, EPHEMERIS_INVALID, EPHEMERIS_VALIDATING,
    EPHEMERIS_VALIDATION_SETTLES_AFTER,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::scheduling_loop::{harness_with_engine, is_uuid, send, Reply, ABSENT_UUID, REGION};

/// A window that excludes no expiration this suite stores: the first second of the epoch
/// to 2100-01-01. `startTime` and `endTime` are required members, so every listing names a
/// window.
const WHOLE_WINDOW: (i64, i64) = (1, 4_102_444_800);

/// An expiration far enough ahead that nothing this suite stores expires while it runs
/// (2096-10-02).
pub(crate) const FUTURE: i64 = 4_000_000_000;

/// An expiration already in the past (2001-09-09), so the ephemeris carrying it is owed
/// `EXPIRED`.
pub(crate) const PAST: i64 = 1_000_000_000;

/// Line 1 of a two-line element set, spelled as the modelled `TleLineOne` requires: 69
/// characters over the pattern `smithy_query(query="shape", shape="TLEData")` carries. A
/// value outside it never reaches the handler — this service's validator answers it.
pub(crate) const TLE_LINE_1: &str = "1 27424U 02022A   24001.50000000  .00001234  00000+0  12345-4 0  9991";

/// Line 2 of the same two-line element set (`TleLineTwo`: 69 characters).
pub(crate) const TLE_LINE_2: &str = "2 27424  98.2000 100.0000 0001234 100.0000 260.0000 14.57100000123454";

/// An OEM body, as the model takes it: "OEM data that you provide directly instead of
/// using an Amazon S3 object" (`smithy_query(query="shape", shape="OEMEphemeris")`), which
/// this emulator stores verbatim rather than parsing.
pub(crate) const OEM_BODY: &str = "CCSDS_OEM_VERS = 2.0\nCREATION_DATE = 2024-01-01T00:00:00\nORIGINATOR = \
                        TEST\n";

/// Longer than the span this service gives an ephemeris in `VALIDATING`, so a read taken
/// after it observes the move the service owed.
///
/// Waited out with `std::thread::sleep` rather than `tokio::time::sleep`: the emulator
/// stores a due instant and never arms a timer, so there is no task to drive while
/// waiting, and this workspace's `tokio` carries no `time` feature.
pub(crate) fn wait_out_validation() {
    std::thread::sleep(EPHEMERIS_VALIDATION_SETTLES_AFTER + Duration::from_millis(600));
}

/// The satellites `setting` onboards, written into this service's store exactly where
/// `state::SATELLITES_ENV` lands them, answering their ids in NORAD-number order.
///
/// Satellites are not created through this API (the model declares no operation that
/// onboards one), so an ephemeris that references one needs the onboarding seeded.
pub(crate) fn onboard(engine: &engine::InMemoryEngine, setting: &str) -> Vec<String> {
    let catalog = parse_satellites(setting);
    let state = service_state(engine).expect("this service's state store");
    state.lock().satellites = catalog.clone();
    let mut onboarded: Vec<(i32, String)> = catalog
        .values()
        .map(|satellite| (satellite.norad_satellite_id, satellite.satellite_id.clone()))
        .collect();
    onboarded.sort();
    onboarded.into_iter().map(|(_, id)| id).collect()
}

/// `CreateEphemeris` with `body`.
async fn create(router: &MultiServiceRouter, body: Value) -> Reply {
    send(router, "POST", "/ephemeris", Some(body)).await
}

/// `CreateEphemeris` with `body`, asserting it was accepted, and the `ephemerisId` it
/// minted.
pub(crate) async fn create_ok(router: &MultiServiceRouter, case: &str, body: Value) -> String {
    let reply = create(router, body).await;
    assert_eq!(
        reply.status, 200,
        "case {case}: expected CreateEphemeris 200, got {} {}",
        reply.status, reply.body
    );
    let ephemeris_id = reply.string("ephemerisId");
    assert!(
        is_uuid(&ephemeris_id),
        "case {case}: an ephemerisId must be a Uuid, got {ephemeris_id:?}"
    );
    ephemeris_id
}

/// A `ListEphemerides` request body: the window every listing must name, plus the members
/// of `extra`.
pub(crate) fn filters(window: (i64, i64), extra: Value) -> Value {
    let mut body = json!({"startTime": window.0, "endTime": window.1});
    for (member, value) in extra.as_object().into_iter().flatten() {
        body[member] = value.clone();
    }
    body
}

/// Every item `ListEphemerides` returns for `request`, across as many pages as it takes,
/// in the order the listing gave them. `page_size` is the caller's `maxResults`, or `None`
/// to name none.
pub(crate) async fn list_all(
    router: &MultiServiceRouter,
    request: &Value,
    page_size: Option<i64>,
) -> Vec<Value> {
    let mut items: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    // One more page than a bounded store could need at a page size of one, so a token
    // that failed to advance is caught as a bounded failure instead of looping forever.
    for page in 0..64 {
        let mut query: Vec<String> = Vec::new();
        if let Some(size) = page_size {
            query.push(format!("maxResults={size}"));
        }
        if let Some(token) = &token {
            query.push(format!("nextToken={token}"));
        }
        let route = if query.is_empty() {
            "/ephemerides".to_string()
        } else {
            format!("/ephemerides?{}", query.join("&"))
        };

        let reply = send(router, "POST", &route, Some(request.clone())).await;
        assert_eq!(
            reply.status, 200,
            "case {request} maxResults={page_size:?} page-{page}: expected 200, got {} {}",
            reply.status, reply.body
        );
        // A page that selected nothing carries no `ephemerides` member at all —
        // `EphemeridesList` is `len 1..500`, so the empty list is not a value the member
        // may take — and a page that selected something carries it as a list.
        let on_page = match reply.body.get("ephemerides") {
            None => Vec::new(),
            Some(member) => {
                let items = member.as_array().unwrap_or_else(|| {
                    panic!(
                        "case {request} maxResults={page_size:?} page-{page}: a present \
                         ephemerides must be a list, got {}",
                        reply.body
                    )
                });
                assert!(
                    !items.is_empty(),
                    "case {request} maxResults={page_size:?} page-{page}: ephemerides is \
                     len 1.., so a page that selected nothing must omit it rather than \
                     answer [], got {}",
                    reply.body
                );
                items.clone()
            }
        };
        if let Some(size) = page_size {
            assert!(
                on_page.len() as i64 <= size,
                "case {request} maxResults={page_size:?} page-{page}: a page must hold at most \
                 {size} items, got {}",
                on_page.len()
            );
        }
        items.extend(on_page);

        token = reply.field("nextToken").as_str().map(str::to_string);
        if token.is_none() {
            return items;
        }
    }
    panic!("case {request}: the listing of /ephemerides never closed");
}

/// The ids `ListEphemerides` reports for `request`, in the order it gave them.
pub(crate) async fn list_ids(router: &MultiServiceRouter, request: &Value) -> Vec<String> {
    list_all(router, request, None)
        .await
        .iter()
        .map(|item| member(item, "ephemerisId"))
        .collect()
}

/// A JSON member of `item`, as a string (empty when absent, so a missing member fails the
/// comparison it is part of rather than panicking here).
fn member(item: &Value, name: &str) -> String {
    item.get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A `tle` ephemeris supplied inline, one `TLEData` entry per `(start, end)` validity
/// range.
pub(crate) fn tle_inline(ranges: &[(i64, i64)]) -> Value {
    let entries: Vec<Value> = ranges
        .iter()
        .map(|(start, end)| {
            json!({
                "tleLine1": TLE_LINE_1,
                "tleLine2": TLE_LINE_2,
                "validTimeRange": {"startTime": start, "endTime": end},
            })
        })
        .collect();
    json!({"tle": {"tleData": entries}})
}

/// An `azEl` ephemeris supplied inline, one segment per `(start, end)` validity range.
///
/// Each segment carries the five time-tagged points `AzElSegment$azElList` requires ("must
/// contain at least five points to support 4th order Lagrange interpolation" —
/// `smithy_query(query="shape", shape="AzElSegment")`).
///
/// The times are epoch seconds, which is how this emulator's schema layer reads a
/// `Timestamp` member of a request body.
pub(crate) fn az_el_inline(ranges: &[(i64, i64)]) -> Value {
    let segments: Vec<Value> = ranges
        .iter()
        .map(|(start, end)| {
            json!({
                "referenceEpoch": start,
                "validTimeRange": {"startTime": start, "endTime": end},
                "azElList": (0..5).map(|step| json!({
                    "dt": f64::from(step) * 10.0,
                    "az": 10.0 + f64::from(step),
                    "el": 20.0 + f64::from(step),
                })).collect::<Vec<Value>>(),
            })
        })
        .collect();
    json!({"azEl": {
        "groundStation": "Ohio 1",
        "data": {"azElData": {"angleUnit": "DEGREE_ANGLE", "azElSegmentList": segments}},
    }})
}

/// An ephemeris of the form the `ProgramTrackSettings` member `member` names — `azEl`,
/// `oem` or `tle`, which are the members of that union and of `EphemerisData` alike
/// (`smithy_query(query="shape", shape="ProgramTrackSettings")`) — created through
/// `CreateEphemeris` on `router`, answering the `ephemerisId` it minted.
///
/// A contact's program track references an ephemeris BY ID, and a reference a write stores
/// has to resolve, so every suite that drives `ReserveContact` or `UpdateContact` with
/// tracking overrides stores the ephemeris those overrides name through the same front
/// door rather than inventing an id for it.
pub(crate) async fn program_track_ephemeris(router: &MultiServiceRouter, member: &str) -> String {
    let data = match member {
        "azEl" => az_el_inline(&[(1_800_000_000, 1_800_086_400)]),
        "oem" => json!({"oem": {"oemData": OEM_BODY}}),
        "tle" => tle_inline(&[(1_800_000_000, 1_800_086_400)]),
        other => panic!("case {other}: not a ProgramTrackSettings member"),
    };
    create_ok(
        router,
        &format!("program track {member}"),
        json!({
            "name": format!("program track {member}"),
            "enabled": true,
            "expirationTime": FUTURE,
            "ephemeris": data,
        }),
    )
    .await
}

/// The Amazon S3 object a suite row supplies its data from.
pub(crate) fn s3_object(key: &str) -> Value {
    json!({"bucket": "ground-station-ephemerides", "key": key, "version": "3HL4kqtJlcpXroDTDmj"})
}

/// One row of the data-form table: the `ephemeris` union member a request may carry, the
/// `ephemerisType` it must be reported under, the `sourceS3Object` it must report, and the
/// expiration its own data implies when the request names none.
struct DataForm {
    case: &'static str,
    data: Value,
    ephemeris_type: &'static str,
    source_s3_object: Option<Value>,
    implied_expiration: Option<i64>,
}

/// Every form of ephemeris data a request may carry: each member of the `EphemerisData`
/// union (`smithy_query(query="shape", shape="EphemerisData")` — `tle`, `oem`, `azEl`),
/// supplied both inline and from an Amazon S3 object, and a TLE set of several entries so
/// the implied expiration is the LAST validity range's end rather than the first's.
fn data_forms() -> Vec<DataForm> {
    vec![
        DataForm {
            case: "tle inline, one entry",
            data: tle_inline(&[(1_800_000_000, 1_800_086_400)]),
            ephemeris_type: "TLE",
            source_s3_object: None,
            implied_expiration: Some(1_800_086_400),
        },
        DataForm {
            case: "tle inline, three entries",
            data: tle_inline(&[
                (1_800_000_000, 1_800_086_400),
                (1_800_086_400, 1_800_172_800),
                (1_800_172_800, 1_800_259_200),
            ]),
            ephemeris_type: "TLE",
            source_s3_object: None,
            implied_expiration: Some(1_800_259_200),
        },
        DataForm {
            case: "tle from an Amazon S3 object",
            data: json!({"tle": {"s3Object": s3_object("tle/aqua.txt")}}),
            ephemeris_type: "TLE",
            source_s3_object: Some(s3_object("tle/aqua.txt")),
            implied_expiration: None,
        },
        DataForm {
            case: "oem inline",
            data: json!({"oem": {"oemData": OEM_BODY}}),
            ephemeris_type: "OEM",
            source_s3_object: None,
            implied_expiration: None,
        },
        DataForm {
            case: "oem from an Amazon S3 object",
            data: json!({"oem": {"s3Object": s3_object("oem/aqua.oem")}}),
            ephemeris_type: "OEM",
            source_s3_object: Some(s3_object("oem/aqua.oem")),
            implied_expiration: None,
        },
        DataForm {
            case: "azEl inline",
            data: az_el_inline(&[
                (1_800_000_000, 1_800_086_400),
                (1_800_086_400, 1_800_172_800),
            ]),
            ephemeris_type: "AZ_EL",
            source_s3_object: None,
            // The end of the last segment's validity.
            implied_expiration: Some(1_800_172_800),
        },
        DataForm {
            case: "azEl from an Amazon S3 object",
            data: json!({"azEl": {
                "groundStation": "Ohio 1",
                "data": {"s3Object": s3_object("azel/ohio.json")},
            }}),
            ephemeris_type: "AZ_EL",
            source_s3_object: Some(s3_object("azel/ohio.json")),
            implied_expiration: None,
        },
    ]
}

/// One ephemeris this suite created: the id it was minted under, the data form it carries
/// and the members the request asked for.
struct Created<'a> {
    id: String,
    form: &'a DataForm,
    priority: Option<i32>,
    enabled: Option<bool>,
    name: String,
}

#[tokio::test]
async fn list_ephemerides_reports_every_member_of_each_ephemeris_it_stored() {
    let (router, engine) = harness_with_engine();
    let satellites = onboard(&engine, "25994=Ohio 1,27424");

    // An account holding no ephemeris lists none: the member is absent — `EphemeridesList`
    // is `len 1..500`, so an empty page is reported by omitting it — and the listing is
    // closed.
    let empty = send(
        &router,
        "POST",
        "/ephemerides",
        Some(filters(WHOLE_WINDOW, json!({}))),
    )
    .await;
    assert_eq!(
        empty.status, 200,
        "case no-ephemerides: expected ListEphemerides 200, got {} {}",
        empty.status, empty.body
    );
    assert!(
        empty.body.get("ephemerides").is_none(),
        "case no-ephemerides: an account holding none must list none, with the member left \
         out rather than empty, got {}",
        empty.body
    );
    assert!(
        empty.field("nextToken").is_null(),
        "case no-ephemerides: a listing with nothing more to come carries no nextToken, got {}",
        empty.body
    );

    // One ephemeris per data form, each differing in name, priority and whether the caller
    // asked for it to be enabled. The priorities include both published bounds of
    // `CustomerEphemerisPriority` (`smithy_query(query="operation",
    // operation="CreateEphemeris")` — range 1..99999) and the absent member, which takes
    // the documented default.
    let forms = data_forms();
    let mut created: Vec<Created<'_>> = Vec::new();
    for (index, form) in forms.iter().enumerate() {
        let priority = [None, Some(1), Some(99_999), Some(42)][index % 4];
        let enabled = [None, Some(true), Some(false)][index % 3];
        let name = format!("ephemeris {index}:{}", form.ephemeris_type);
        let mut body = json!({
            "name": name,
            "satelliteId": satellites[index % satellites.len()],
            "expirationTime": FUTURE,
            "ephemeris": form.data,
        });
        if let Some(priority) = priority {
            body["priority"] = json!(priority);
        }
        if let Some(enabled) = enabled {
            body["enabled"] = json!(enabled);
        }
        let id = create_ok(&router, form.case, body).await;
        created.push(Created {
            id,
            form,
            priority,
            enabled,
            name,
        });
    }

    // Every ephemeris is addressed by an id of its own.
    let mut ids: Vec<String> = created.iter().map(|row| row.id.clone()).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(
        ids.len(),
        forms.len(),
        "case distinct-ids: every create must mint its own ephemerisId, got {ids:?}"
    );

    wait_out_validation();
    let items = list_all(&router, &filters(WHOLE_WINDOW, json!({})), None).await;
    assert_eq!(
        items.len(),
        created.len(),
        "case whole-listing: expected one item per stored ephemeris, got {items:?}"
    );

    // The listing reports the ephemerides in creation order, which is the order they were
    // created in here.
    let reported: Vec<String> = items
        .iter()
        .map(|item| member(item, "ephemerisId"))
        .collect();
    let expected_order: Vec<String> = created.iter().map(|row| row.id.clone()).collect();
    assert_eq!(
        reported, expected_order,
        "case ordering: the listing must report ephemerides in creation order, got {reported:?}"
    );

    for (item, row) in items.iter().zip(&created) {
        let Created {
            id,
            form,
            priority,
            enabled,
            name,
        } = row;
        let case = form.case;
        assert_eq!(
            member(item, "ephemerisId"),
            *id,
            "case {case}: the listing must report the minted ephemerisId, got {item}"
        );
        assert_eq!(
            member(item, "ephemerisType"),
            form.ephemeris_type,
            "case {case}: the type is the EphemerisData member the request set, got {item}"
        );
        assert_eq!(
            member(item, "name"),
            *name,
            "case {case}: expected the stored name {name:?}, got {item}"
        );
        assert_eq!(
            item.get("priority").and_then(Value::as_i64),
            Some(i64::from(
                priority.unwrap_or(DEFAULT_EPHEMERIS_PRIORITY)
            )),
            "case {case}: expected priority {priority:?} (default \
             {DEFAULT_EPHEMERIS_PRIORITY}), got {item}"
        );
        assert_eq!(
            item.get("enabled").and_then(Value::as_bool),
            Some(enabled.unwrap_or(false)),
            "case {case}: expected enabled {enabled:?} (absent means not enabled), got {item}"
        );
        assert_eq!(
            member(item, "status"),
            if enabled.unwrap_or(false) {
                EPHEMERIS_ENABLED
            } else {
                EPHEMERIS_DISABLED
            },
            "case {case}: a validated ephemeris carries the status its enabled asked for, \
             got {item}"
        );
        match &form.source_s3_object {
            None => assert!(
                item.get("sourceS3Object").is_none_or(Value::is_null),
                "case {case}: data supplied inline has no source Amazon S3 object, got {item}"
            ),
            Some(object) => assert_eq!(
                item.get("sourceS3Object"),
                Some(object),
                "case {case}: expected the supplied Amazon S3 object {object}, got {item}"
            ),
        }
        let creation_time = item
            .get("creationTime")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("case {case}: creationTime is not a number in {item}"));
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("the clock is past the epoch")
            .as_secs_f64();
        assert!(
            (now - creation_time).abs() < 600.0,
            "case {case}: the creationTime must be when the ephemeris was uploaded, got \
             {creation_time} against {now}"
        );
    }

    // Each ephemeris is in the store under its own id, with the ARN the one index resolves
    // to it — what the tagging operations address it by.
    {
        let state = service_state(&*engine).expect("this service's state store");
        let guard = state.lock();
        for row in &created {
            let (id, form) = (&row.id, row.form);
            let stored = guard
                .ephemerides
                .get(id)
                .unwrap_or_else(|| panic!("case {}: {id} must be in the store", form.case));
            let expected_arn = format!(
                "arn:aws:groundstation:{REGION}:{}:ephemeris/{id}",
                engine::emulator_account_id()
            );
            assert_eq!(
                stored.ephemeris_arn, expected_arn,
                "case {}: expected the ephemeris ARN {expected_arn}, got {:?}",
                form.case, stored.ephemeris_arn
            );
            assert_eq!(
                guard.arn_index.get(&stored.ephemeris_arn),
                Some(&ResourceRef::Ephemeris {
                    ephemeris_id: id.clone()
                }),
                "case {}: the ARN index must resolve the ephemeris ARN, got {:?}",
                form.case,
                guard.arn_index.get(&stored.ephemeris_arn)
            );
        }
    }

    // A listing is a REPORT: reading it twice reports the same store.
    assert_eq!(
        items,
        list_all(&router, &filters(WHOLE_WINDOW, json!({})), None).await,
        "case repeated-read: a listing must not change what the next caller sees"
    );
}

#[tokio::test]
async fn an_ephemeris_whose_request_names_no_expiration_takes_its_data_s_own() {
    let (router, engine) = harness_with_engine();
    let _ = onboard(&engine, "25994");

    // The window the listing names selects on the EXPIRATION time, so an ephemeris created
    // without one is listed under the end of its own data's validity, and one whose data
    // declares no validity at all carries no expiration time for the window to select on,
    // so no window lists it.
    for form in data_forms() {
        let id = create_ok(
            &router,
            form.case,
            json!({"name": "implied expiration", "ephemeris": form.data}),
        )
        .await;
        wait_out_validation();

        match form.implied_expiration {
            Some(expiration) => {
                let inside = list_ids(&router, &filters((expiration - 60, expiration), json!({})))
                    .await;
                assert_eq!(
                    inside,
                    vec![id.clone()],
                    "case {}: the ephemeris must expire at the end of its data's validity \
                     ({expiration}), got {inside:?}",
                    form.case
                );
                let outside =
                    list_ids(&router, &filters((expiration + 1, expiration + 60), json!({}))).await;
                assert!(
                    outside.is_empty(),
                    "case {}: a window past that expiration must select nothing, got {outside:?}",
                    form.case
                );
            }
            None => {
                for window in [(1_i64, 2_i64), (PAST, PAST + 60), WHOLE_WINDOW] {
                    let listed = list_ids(&router, &filters(window, json!({}))).await;
                    assert!(
                        listed.is_empty(),
                        "case {} window {window:?}: an ephemeris carrying no expiration time \
                         has none inside the window, so no window selects it, got {listed:?}",
                        form.case
                    );
                }
            }
        }

        // Each row stands on its own, so the store starts empty for the next one.
        service_state(&*engine)
            .expect("this service's state store")
            .lock()
            .ephemerides
            .clear();
    }
}

/// One row of the status table: what the request asked for, and the `EphemerisStatus` the
/// ephemeris must carry once the validation the create started has fallen due.
struct StatusRow {
    case: &'static str,
    body: Value,
    settled_status: &'static str,
}

#[tokio::test]
async fn an_ephemeris_moves_out_of_validating_to_the_status_its_data_and_request_earn() {
    let (router, _engine) = harness_with_engine();

    // Every `EphemerisStatus` this service puts a customer ephemeris in
    // (`smithy_query(query="shape", shape="EphemerisStatus")` — VALIDATING, INVALID,
    // ERROR, ENABLED, DISABLED, EXPIRED). `ERROR` is the service's own processing failure,
    // which nothing in this emulator suffers, so no row claims it.
    let rows = vec![
        StatusRow {
            case: "asked to be enabled",
            body: json!({
                "name": "enabled ephemeris",
                "enabled": true,
                "expirationTime": FUTURE,
                "ephemeris": tle_inline(&[(1_800_000_000, 1_800_086_400)]),
            }),
            settled_status: EPHEMERIS_ENABLED,
        },
        StatusRow {
            case: "asked to stay disabled",
            body: json!({
                "name": "disabled ephemeris",
                "enabled": false,
                "expirationTime": FUTURE,
                "ephemeris": tle_inline(&[(1_800_000_000, 1_800_086_400)]),
            }),
            settled_status: EPHEMERIS_DISABLED,
        },
        StatusRow {
            case: "asked for neither",
            body: json!({
                "name": "default ephemeris",
                "expirationTime": FUTURE,
                "ephemeris": tle_inline(&[(1_800_000_000, 1_800_086_400)]),
            }),
            settled_status: EPHEMERIS_DISABLED,
        },
        StatusRow {
            case: "a tle validity range that does not run forwards",
            body: json!({
                "name": "backwards tle",
                "enabled": true,
                "expirationTime": FUTURE,
                "ephemeris": tle_inline(&[(1_800_086_400, 1_800_000_000)]),
            }),
            settled_status: EPHEMERIS_INVALID,
        },
        StatusRow {
            case: "an azEl segment that does not run forwards",
            body: json!({
                "name": "backwards azEl",
                "enabled": true,
                "expirationTime": FUTURE,
                "ephemeris": az_el_inline(&[(1_800_086_400, 1_800_000_000)]),
            }),
            settled_status: EPHEMERIS_INVALID,
        },
        StatusRow {
            case: "an expiration already in the past",
            body: json!({
                "name": "expired ephemeris",
                "enabled": true,
                "expirationTime": PAST,
                "ephemeris": tle_inline(&[(1_800_000_000, 1_800_086_400)]),
            }),
            settled_status: EPHEMERIS_EXPIRED,
        },
    ];

    let mut created: Vec<(String, &StatusRow)> = Vec::new();
    for row in &rows {
        let id = create_ok(&router, row.case, row.body.clone()).await;
        created.push((id, row));
    }

    // The validation span is never zero, so a caller reading straight after the create
    // still observes the transient status.
    let validating = list_all(&router, &filters(WHOLE_WINDOW, json!({})), None).await;
    for (item, (id, row)) in validating.iter().zip(&created) {
        assert_eq!(
            member(item, "ephemerisId"),
            *id,
            "case {}: the listing must report the created ephemeris, got {item}",
            row.case
        );
        assert_eq!(
            member(item, "status"),
            EPHEMERIS_VALIDATING,
            "case {}: an ephemeris is admitted in {EPHEMERIS_VALIDATING}, got {item}",
            row.case
        );
    }

    wait_out_validation();
    let settled = list_all(&router, &filters(WHOLE_WINDOW, json!({})), None).await;
    for (item, (id, row)) in settled.iter().zip(&created) {
        assert_eq!(
            member(item, "ephemerisId"),
            *id,
            "case {}: the listing must report the created ephemeris, got {item}",
            row.case
        );
        assert_eq!(
            member(item, "status"),
            row.settled_status,
            "case {}: expected {} once the validation fell due, got {item}",
            row.case,
            row.settled_status
        );
    }

    // And the statuses hold: reading again reports the same store.
    assert_eq!(
        settled,
        list_all(&router, &filters(WHOLE_WINDOW, json!({})), None).await,
        "case repeated-read: a settled status must not move again on the next read"
    );
}

#[tokio::test]
async fn list_ephemerides_returns_exactly_the_ephemerides_its_filters_select() {
    let (router, engine) = harness_with_engine();
    let satellites = onboard(&engine, "25994=Ohio 1,27424");
    let (first_satellite, second_satellite) = (satellites[0].clone(), satellites[1].clone());

    // A store spanning the filters: two satellites, three ephemeris types, enabled and
    // disabled, and expirations at the window edges this suite asks about.
    let edge = 2_000_000_000_i64;
    let enabled_tle = create_ok(
        &router,
        "enabled tle",
        json!({
            "name": "enabled tle",
            "satelliteId": first_satellite,
            "enabled": true,
            "expirationTime": edge,
            "ephemeris": tle_inline(&[(1_800_000_000, 1_800_086_400)]),
        }),
    )
    .await;
    let disabled_oem = create_ok(
        &router,
        "disabled oem",
        json!({
            "name": "disabled oem",
            "satelliteId": first_satellite,
            "enabled": false,
            "expirationTime": edge + 3600,
            "ephemeris": json!({"oem": {"oemData": OEM_BODY}}),
        }),
    )
    .await;
    let other_satellite_az_el = create_ok(
        &router,
        "azEl of the other satellite",
        json!({
            "name": "azEl of the other satellite",
            "satelliteId": second_satellite,
            "enabled": true,
            "expirationTime": edge + 7200,
            "ephemeris": az_el_inline(&[(1_800_000_000, 1_800_086_400)]),
        }),
    )
    .await;
    let unattached_tle = create_ok(
        &router,
        "a tle attached to no satellite",
        json!({
            "name": "unattached tle",
            "enabled": true,
            "expirationTime": edge + 10_800,
            "ephemeris": tle_inline(&[(1_800_000_000, 1_800_086_400)]),
        }),
    )
    .await;
    wait_out_validation();

    let everything = vec![
        enabled_tle.clone(),
        disabled_oem.clone(),
        other_satellite_az_el.clone(),
        unattached_tle.clone(),
    ];

    // Each row names a filter set and the ids it must select, in the listing's order.
    let rows: Vec<(&str, Value, Vec<String>)> = vec![
        ("no filter but the window", json!({}), everything.clone()),
        (
            "an empty status list narrows nothing",
            json!({"statusList": []}),
            everything.clone(),
        ),
        (
            "one status",
            json!({"statusList": [EPHEMERIS_DISABLED]}),
            vec![disabled_oem.clone()],
        ),
        (
            "several statuses",
            json!({"statusList": [EPHEMERIS_ENABLED, EPHEMERIS_DISABLED]}),
            everything.clone(),
        ),
        (
            "a status nothing holds",
            json!({"statusList": [EPHEMERIS_EXPIRED]}),
            Vec::new(),
        ),
        (
            "type TLE",
            json!({"ephemerisType": "TLE"}),
            vec![enabled_tle.clone(), unattached_tle.clone()],
        ),
        (
            "type OEM",
            json!({"ephemerisType": "OEM"}),
            vec![disabled_oem.clone()],
        ),
        (
            "type AZ_EL",
            json!({"ephemerisType": "AZ_EL"}),
            vec![other_satellite_az_el.clone()],
        ),
        (
            // The fourth `EphemerisType` value belongs to an ephemeris AWS supplies
            // itself, which no customer request creates.
            "type SERVICE_MANAGED",
            json!({"ephemerisType": "SERVICE_MANAGED"}),
            Vec::new(),
        ),
        (
            "one satellite",
            json!({"satelliteId": first_satellite}),
            vec![enabled_tle.clone(), disabled_oem.clone()],
        ),
        (
            "the other satellite",
            json!({"satelliteId": second_satellite}),
            vec![other_satellite_az_el.clone()],
        ),
        (
            "satellite and type together",
            json!({"satelliteId": first_satellite, "ephemerisType": "TLE"}),
            vec![enabled_tle.clone()],
        ),
        (
            "satellite, type and status together",
            json!({
                "satelliteId": first_satellite,
                "ephemerisType": "TLE",
                "statusList": [EPHEMERIS_DISABLED],
            }),
            Vec::new(),
        ),
    ];

    for (case, filter, expected) in &rows {
        let listed = list_ids(&router, &filters(WHOLE_WINDOW, filter.clone())).await;
        assert_eq!(
            listed, *expected,
            "case {case} ({filter}): expected {expected:?}, got {listed:?}"
        );
    }

    // The window is tested against the expiration time, and both of its edges are
    // INSIDE it: an expiration equal to `startTime` or to `endTime` is within the range
    // the request named, one a second outside either edge is not.
    let window_rows: Vec<(&str, (i64, i64), Vec<String>)> = vec![
        (
            "the window starts at the first expiration",
            (edge, edge + 10_800),
            everything.clone(),
        ),
        (
            "the window ends at the first expiration",
            (edge - 3600, edge),
            vec![enabled_tle.clone()],
        ),
        (
            "the window starts one second past the first expiration",
            (edge + 1, edge + 10_800),
            vec![
                disabled_oem.clone(),
                other_satellite_az_el.clone(),
                unattached_tle.clone(),
            ],
        ),
        (
            "the window ends one second before the first expiration",
            (edge - 3600, edge - 1),
            Vec::new(),
        ),
        (
            "a window of one instant, on an expiration",
            (edge, edge),
            vec![enabled_tle.clone()],
        ),
        (
            "a window past every expiration",
            (edge + 10_801, edge + 100_000),
            Vec::new(),
        ),
    ];

    for (case, window, expected) in &window_rows {
        let listed = list_ids(&router, &filters(*window, json!({}))).await;
        assert_eq!(
            listed, *expected,
            "case {case} ({window:?}): expected {expected:?}, got {listed:?}"
        );
    }

    // A window that runs backwards describes no interval at all, so it is refused rather
    // than answered with the empty page it would select.
    let backwards = send(
        &router,
        "POST",
        "/ephemerides",
        Some(filters((edge + 1, edge), json!({}))),
    )
    .await;
    assert_eq!(
        backwards.status, 431,
        "case backwards-window: expected 431, got {} {}",
        backwards.status, backwards.body
    );
    assert_eq!(
        backwards.error_code(),
        "InvalidParameterException",
        "case backwards-window: wrong error shape, got {}",
        backwards.body
    );

    // A satellite this account is not on-boarded to references nothing, so the filter is
    // answered as the missing resource it names.
    let unknown = send(
        &router,
        "POST",
        "/ephemerides",
        Some(filters(WHOLE_WINDOW, json!({"satelliteId": ABSENT_UUID}))),
    )
    .await;
    assert_eq!(
        unknown.status, 434,
        "case unknown-satellite: expected 434, got {} {}",
        unknown.status, unknown.body
    );
    assert_eq!(
        unknown.error_code(),
        "ResourceNotFoundException",
        "case unknown-satellite: wrong error shape, got {}",
        unknown.body
    );
    assert_eq!(
        unknown.field("message").as_str(),
        Some(satellite_not_found_message(ABSENT_UUID).as_str()),
        "case unknown-satellite: expected this service's satellite text, got {}",
        unknown.body
    );
}

#[tokio::test]
async fn list_ephemerides_pages_through_every_ephemeris_once_whatever_the_page_size() {
    let (router, _engine) = harness_with_engine();

    // Six ephemerides, so a page size can divide the sequence evenly, unevenly, exactly,
    // or not bound it at all.
    for index in 0..6 {
        create_ok(
            &router,
            "paging",
            json!({
                "name": format!("page member {index}"),
                "enabled": true,
                "expirationTime": FUTURE + index,
                "ephemeris": tle_inline(&[(1_800_000_000, 1_800_086_400)]),
            }),
        )
        .await;
    }
    wait_out_validation();

    let request = filters(WHOLE_WINDOW, json!({}));
    let whole: Vec<String> = list_all(&router, &request, None)
        .await
        .iter()
        .map(|item| member(item, "ephemerisId"))
        .collect();
    assert_eq!(
        whole.len(),
        6,
        "case whole-listing: expected every stored ephemeris, got {whole:?}"
    );

    for size in [1_i64, 2, 3, 4, 5, 6, 7, 100] {
        let paged: Vec<String> = list_all(&router, &request, Some(size))
            .await
            .iter()
            .map(|item| member(item, "ephemerisId"))
            .collect();
        assert_eq!(
            paged, whole,
            "case maxResults={size}: paging must yield every ephemeris exactly once, in the \
             listing's order, got {paged:?}"
        );
    }

    // Each published bound of `PaginationMaxResults` and one value past it (source:
    // `smithy_query(query="operation", operation="ListEphemerides")` — range 1..100). A
    // value the model refuses never reaches the handler: this service's validator answers
    // it.
    for (case, size, accepted) in [
        ("the lower bound", 1_i64, true),
        ("the upper bound", 100, true),
        ("one below the lower bound", 0, false),
        ("one above the upper bound", 101, false),
    ] {
        let reply = send(
            &router,
            "POST",
            &format!("/ephemerides?maxResults={size}"),
            Some(request.clone()),
        )
        .await;
        if accepted {
            assert_eq!(
                reply.status, 200,
                "case maxResults={size} ({case}): expected 200, got {} {}",
                reply.status, reply.body
            );
        } else {
            assert_eq!(
                reply.status, 400,
                "case maxResults={size} ({case}): expected a refusal, got {} {}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error_code(),
                BAD_REQUEST_ERROR_CODE,
                "case maxResults={size} ({case}): expected {BAD_REQUEST_ERROR_CODE}, got {}",
                reply.body
            );
        }
    }

    // A token addresses an offset into the sequence that issued it, so this listing
    // accepts only its own, and refuses anything else rather than resuming at an offset
    // into an unrelated sequence.
    let own_token = send(
        &router,
        "POST",
        "/ephemerides?maxResults=1",
        Some(request.clone()),
    )
    .await
    .field("nextToken")
    .as_str()
    .map(str::to_string)
    .expect("a capped page hands back a token");
    let resumed = send(
        &router,
        "POST",
        &format!("/ephemerides?maxResults=1&nextToken={own_token}"),
        Some(request.clone()),
    )
    .await;
    assert_eq!(
        resumed.status, 200,
        "case own-token: a token this listing issued must resume it, got {} {}",
        resumed.status, resumed.body
    );

    for (case, token, request) in [
        (
            "a token this service never issued",
            "not-a-token".to_string(),
            request.clone(),
        ),
        (
            "a token naming no offset",
            "ephemerides.not-a-number".to_string(),
            request.clone(),
        ),
        (
            "a token of another listing's name",
            "satellites.1".to_string(),
            request.clone(),
        ),
        (
            // The offset counts within the FILTERED sequence, so a token issued under one
            // filter set does not address an offset into another.
            "this listing's own token under different filters",
            own_token.clone(),
            filters(WHOLE_WINDOW, json!({"ephemerisType": "TLE"})),
        ),
    ] {
        let reply = send(
            &router,
            "POST",
            &format!("/ephemerides?maxResults=1&nextToken={token}"),
            Some(request),
        )
        .await;
        assert_eq!(
            reply.status, 431,
            "case {case} ({token}): expected 431, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "InvalidParameterException",
            "case {case} ({token}): wrong error shape, got {}",
            reply.body
        );
    }
}

#[tokio::test]
async fn create_ephemeris_refuses_a_request_that_names_no_one_source_of_data() {
    let (router, engine) = harness_with_engine();

    // Data is supplied EITHER inline OR from an Amazon S3 object, and an ephemeris without
    // data is no ephemeris at all. Each row is refused with the operation's own modelled
    // `InvalidParameterException` (HTTP 431), naming the member at fault.
    let rows: Vec<(&str, Value)> = vec![
        ("no ephemeris data at all", json!({"name": "no data"})),
        (
            "tle supplied both inline and from Amazon S3",
            json!({"name": "two sources", "ephemeris": {"tle": {
                "tleData": [{
                    "tleLine1": TLE_LINE_1,
                    "tleLine2": TLE_LINE_2,
                    "validTimeRange": {"startTime": 1_800_000_000_i64, "endTime": 1_800_086_400_i64},
                }],
                "s3Object": s3_object("tle/aqua.txt"),
            }}}),
        ),
        (
            "tle supplied neither way",
            json!({"name": "no source", "ephemeris": {"tle": {}}}),
        ),
        (
            "oem supplied both inline and from Amazon S3",
            json!({"name": "two sources", "ephemeris": {"oem": {
                "oemData": OEM_BODY,
                "s3Object": s3_object("oem/aqua.oem"),
            }}}),
        ),
        (
            "oem supplied neither way",
            json!({"name": "no source", "ephemeris": {"oem": {}}}),
        ),
    ];

    for (case, body) in &rows {
        let reply = create(&router, body.clone()).await;
        assert_eq!(
            reply.status, 431,
            "case {case}: expected 431, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "InvalidParameterException",
            "case {case}: wrong error shape, got {}",
            reply.body
        );
        assert_eq!(
            reply.field("parameterName").as_str(),
            Some("ephemeris"),
            "case {case}: the refusal must name the member at fault, got {}",
            reply.body
        );
    }

    // A satellite this account is not on-boarded to references nothing.
    let unknown = create(
        &router,
        json!({
            "name": "unknown satellite",
            "satelliteId": ABSENT_UUID,
            "ephemeris": tle_inline(&[(1_800_000_000, 1_800_086_400)]),
        }),
    )
    .await;
    assert_eq!(
        unknown.status, 434,
        "case unknown-satellite: expected 434, got {} {}",
        unknown.status, unknown.body
    );
    assert_eq!(
        unknown.field("message").as_str(),
        Some(satellite_not_found_message(ABSENT_UUID).as_str()),
        "case unknown-satellite: expected this service's satellite text, got {}",
        unknown.body
    );

    // Not one of those requests stored anything: a refused create leaves the store as it
    // found it.
    let listed = list_ids(&router, &filters(WHOLE_WINDOW, json!({}))).await;
    assert!(
        listed.is_empty(),
        "case refusals-store-nothing: a refused create must store nothing, got {listed:?}"
    );

    // The modelled bounds of the members the request carries, each published bound and one
    // value past it (`smithy_query(query="operation", operation="CreateEphemeris")` —
    // `name` is a `SafeName` of 1..256 characters over `^[ a-zA-Z0-9_:-]{1,256}$`,
    // `priority` a `CustomerEphemerisPriority` in 1..99999). A value the model refuses
    // never reaches the handler.
    let data = tle_inline(&[(1_800_000_000, 1_800_086_400)]);
    let longest_name = "n".repeat(256);
    let too_long_name = "n".repeat(257);
    let member_rows: Vec<(&str, Value, bool)> = vec![
        ("the shortest name", json!({"name": "a"}), true),
        ("the longest name", json!({"name": longest_name}), true),
        ("an empty name", json!({"name": ""}), false),
        (
            "a name one character too long",
            json!({"name": too_long_name}),
            false,
        ),
        (
            "a name with a character the pattern excludes",
            json!({"name": "ephemeris/one"}),
            false,
        ),
        (
            "the lowest priority",
            json!({"name": "priority", "priority": 1}),
            true,
        ),
        (
            "the highest priority",
            json!({"name": "priority", "priority": 99_999}),
            true,
        ),
        (
            "one below the lowest priority",
            json!({"name": "priority", "priority": 0}),
            false,
        ),
        (
            "one above the highest priority",
            json!({"name": "priority", "priority": 100_000}),
            false,
        ),
    ];

    for (case, members, accepted) in &member_rows {
        let mut body = json!({"ephemeris": data.clone()});
        for (name, value) in members.as_object().into_iter().flatten() {
            body[name] = value.clone();
        }
        let reply = create(&router, body).await;
        if *accepted {
            assert_eq!(
                reply.status, 200,
                "case {case}: expected 200, got {} {}",
                reply.status, reply.body
            );
        } else {
            assert_eq!(
                reply.status, 400,
                "case {case}: expected a refusal, got {} {}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error_code(),
                BAD_REQUEST_ERROR_CODE,
                "case {case}: expected {BAD_REQUEST_ERROR_CODE}, got {}",
                reply.body
            );
        }
    }

    // The tagging rules are the platform's, and they are refused before anything is
    // stored: a reserved tag key is not one a caller may set.
    let reserved: HashMap<&str, &str> = HashMap::from([("aws:owner", "ground-station")]);
    let tagged = create(
        &router,
        json!({
            "name": "reserved tag",
            "tags": reserved,
            "ephemeris": tle_inline(&[(1_800_000_000, 1_800_086_400)]),
        }),
    )
    .await;
    assert_eq!(
        tagged.status, 431,
        "case reserved-tag: expected 431, got {} {}",
        tagged.status, tagged.body
    );
    assert_eq!(
        tagged.error_code(),
        "InvalidParameterException",
        "case reserved-tag: wrong error shape, got {}",
        tagged.body
    );

    // And the store still holds only what was accepted above.
    let stored = service_state(&*engine)
        .expect("this service's state store")
        .lock()
        .ephemerides
        .len();
    assert_eq!(
        stored, 4,
        "case accepted-only: only the four accepted creates may be in the store, got {stored}"
    );
}
