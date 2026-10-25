// SPDX-License-Identifier: CC0-1.0

//! `DescribeEphemeris` (`GET /ephemeris/{ephemerisId}`) end to end over HTTP, through the
//! same front door as the rest of this target's suites — the read that observes what
//! `CreateEphemeris` (`POST /ephemeris`) persisted.
//!
//! The RULE under test: `DescribeEphemeris` REPORTS the ephemeris this account holds under
//! the `{ephemerisId}` it is addressed by — that ephemeris's own `satelliteId`, `name`,
//! `priority`, `enabled`, `creationTime`, `tags`, the `EphemerisStatus` in force now, and
//! its supplied data under the `EphemerisTypeDescription` member the data was created
//! under, carrying the Amazon S3 object it names or the data itself when it was supplied
//! inline — naming an `invalidReason` only while that status is `INVALID`, refusing an id
//! that names no ephemeris, and changing nothing a later caller sees.
//!
//! So the suites below assert that rule over SETS of inputs rather than over one call:
//!
//! * a table spanning every `EphemerisData` member a request may carry (`tle`, `oem`,
//!   `azEl` — `smithy_query(query="shape", shape="EphemerisData")`), each supplied both
//!   inline and from an Amazon S3 object, differing in name, priority (each published
//!   bound of `CustomerEphemerisPriority` among them, and the absent member that takes the
//!   documented default), whether the caller asked for it to be enabled, whether it names
//!   a satellite, and how it is tagged — each read back and compared member by member with
//!   what was created;
//! * the status an ephemeris is reported in, over a table spanning every `EphemerisStatus`
//!   this service puts a customer ephemeris in, with `invalidReason` asserted present only
//!   where that status is `INVALID` — including on the rows owed `INVALID`, which must
//!   name no reason while they are still `VALIDATING`;
//! * the refusal of an id that names no ephemeris, over several well-formed `Uuid`s, and of
//!   an id outside the modelled `Uuid` pattern, neither of which disturbs the store.

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use service_com_amazonaws_groundstation_groundstation::state::{
    BAD_REQUEST_ERROR_CODE, DEFAULT_EPHEMERIS_PRIORITY, EPHEMERIS_DISABLED, EPHEMERIS_ENABLED,
    EPHEMERIS_ENABLED_BY_DEFAULT, EPHEMERIS_EXPIRED, EPHEMERIS_INVALID, EPHEMERIS_NOT_FOUND,
    EPHEMERIS_TIME_RANGE_INVALID, EPHEMERIS_VALIDATING, INVALID_REQUEST_BODY,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::ephemeris_catalog::{
    az_el_inline, create_ok, onboard, s3_object, tle_inline, wait_out_validation, FUTURE, OEM_BODY,
    PAST, TLE_LINE_1, TLE_LINE_2,
};
use crate::scheduling_loop::{harness_with_engine, send, Reply, ABSENT_UUID};

/// `DescribeEphemeris` for `ephemeris_id`.
///
/// `pub(crate)` so the sibling `ephemeris_updates.rs` suite reads an ephemeris back
/// through the same front door rather than standing up a second copy of this call.
pub(crate) async fn describe(router: &MultiServiceRouter, ephemeris_id: &str) -> Reply {
    send(router, "GET", &format!("/ephemeris/{ephemeris_id}"), None).await
}

/// `DescribeEphemeris` for `ephemeris_id`, asserting it was answered, and the body it
/// answered with.
pub(crate) async fn describe_ok(router: &MultiServiceRouter, case: &str, ephemeris_id: &str) -> Value {
    let reply = describe(router, ephemeris_id).await;
    assert_eq!(
        reply.status, 200,
        "case {case}: expected DescribeEphemeris 200, got {} {}",
        reply.status, reply.body
    );
    reply.body
}

/// A JSON member of `body`, as a string (empty when absent, so a missing member fails the
/// comparison it is part of rather than panicking here).
fn text(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The instants an epoch-second `Timestamp` member may legitimately carry for something
/// created during this test: now, in whole seconds.
fn now_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the test clock is after the epoch")
        .as_secs_f64()
}

/// What a row's `suppliedData` must carry inside its union member.
enum ExpectedData {
    /// Supplied from an Amazon S3 object: that object, and no inline data.
    FromS3(Value),
    /// Supplied inline, and described verbatim — the text the request itself carried.
    Verbatim(String),
    /// Supplied inline as az/el segments, and described as the JSON document the model
    /// gives that data: this angle unit, and one object per validity range (epoch seconds,
    /// described in the ISO-8601 UTC form the members' `@timestampFormat` names) carrying
    /// this many time-tagged points.
    AzElJson {
        angle_unit: &'static str,
        ranges: Vec<(i64, i64)>,
        points_per_segment: usize,
    },
}

/// One row of the data-form table: a form of ephemeris data a request may carry, the
/// `EphemerisTypeDescription` member it must be described under, what that description
/// must hold, and the members the request asks for beside the data.
struct Row {
    case: &'static str,
    data: Value,
    member: &'static str,
    expected: ExpectedData,
    priority: Option<i32>,
    enabled: Option<bool>,
    satellite: bool,
    tags: Value,
}

/// Every form of ephemeris data a request may carry, each read back under the member it
/// was created under.
///
/// The three union members are the model's own (`smithy_query(query="shape",
/// shape="EphemerisData")` — `tle`, `oem`, `azEl`), each supplied inline and from an Amazon
/// S3 object. The priorities span both published bounds of `CustomerEphemerisPriority`
/// (range 1..99999) plus the absent member, which takes the default `CreateEphemeris`
/// documents; the tags span none, one and several; and the TLE rows span one entry and
/// three, so the description of a set is every line of it rather than its first.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "tle inline, one entry",
            data: tle_inline(&[(1_800_000_000, 1_800_086_400)]),
            member: "tle",
            expected: ExpectedData::Verbatim(format!("{TLE_LINE_1}\n{TLE_LINE_2}")),
            priority: None,
            enabled: None,
            satellite: true,
            tags: json!({}),
        },
        Row {
            case: "tle inline, three entries",
            data: tle_inline(&[
                (1_800_000_000, 1_800_086_400),
                (1_800_086_400, 1_800_172_800),
                (1_800_172_800, 1_800_259_200),
            ]),
            member: "tle",
            expected: ExpectedData::Verbatim(
                [TLE_LINE_1, TLE_LINE_2, TLE_LINE_1, TLE_LINE_2, TLE_LINE_1, TLE_LINE_2]
                    .join("\n"),
            ),
            priority: Some(1),
            enabled: Some(true),
            satellite: false,
            tags: json!({"mission": "aqua"}),
        },
        Row {
            case: "tle from an Amazon S3 object",
            data: json!({"tle": {"s3Object": s3_object("tle/aqua.txt")}}),
            member: "tle",
            expected: ExpectedData::FromS3(s3_object("tle/aqua.txt")),
            priority: Some(99_999),
            enabled: Some(false),
            satellite: true,
            tags: json!({"mission": "aqua", "owner": "flight-ops"}),
        },
        Row {
            case: "oem inline",
            data: json!({"oem": {"oemData": OEM_BODY}}),
            member: "oem",
            expected: ExpectedData::Verbatim(OEM_BODY.to_string()),
            priority: Some(42),
            enabled: Some(true),
            satellite: true,
            tags: json!({}),
        },
        Row {
            case: "oem from an Amazon S3 object",
            data: json!({"oem": {"s3Object": s3_object("oem/aqua.oem")}}),
            member: "oem",
            expected: ExpectedData::FromS3(s3_object("oem/aqua.oem")),
            priority: None,
            enabled: Some(false),
            satellite: false,
            tags: json!({"mission": "terra"}),
        },
        Row {
            case: "azEl inline, two segments",
            data: az_el_inline(&[
                (1_800_000_000, 1_800_086_400),
                (1_800_086_400, 1_800_172_800),
            ]),
            member: "azEl",
            expected: ExpectedData::AzElJson {
                angle_unit: "DEGREE_ANGLE",
                ranges: vec![
                    (1_800_000_000, 1_800_086_400),
                    (1_800_086_400, 1_800_172_800),
                ],
                points_per_segment: 5,
            },
            priority: Some(7),
            enabled: Some(true),
            satellite: true,
            tags: json!({"ground-station": "Ohio 1"}),
        },
        Row {
            case: "azEl from an Amazon S3 object",
            data: json!({"azEl": {
                "groundStation": "Ohio 1",
                "data": {"s3Object": s3_object("azel/ohio.json")},
            }}),
            member: "azEl",
            expected: ExpectedData::FromS3(s3_object("azel/ohio.json")),
            priority: Some(2),
            enabled: None,
            satellite: false,
            tags: json!({}),
        },
    ]
}

/// Assert `supplied_data` describes `row`'s data: under the member it was created under,
/// and carrying what that form of data is described by.
fn assert_supplied_data(row: &Row, supplied_data: &Value) {
    let members: Vec<&String> = supplied_data
        .as_object()
        .unwrap_or_else(|| {
            panic!(
                "case {}: suppliedData must be an EphemerisTypeDescription union, got \
                 {supplied_data}",
                row.case
            )
        })
        .keys()
        .collect();
    assert_eq!(
        members,
        vec![row.member],
        "case {}: suppliedData must carry exactly the union member the data was created \
         under, got {supplied_data}",
        row.case
    );
    let description = &supplied_data[row.member];

    match &row.expected {
        ExpectedData::FromS3(object) => {
            assert_eq!(
                description.get("sourceS3Object"),
                Some(object),
                "case {}: data supplied from an Amazon S3 object is described by that \
                 object, got {description}",
                row.case
            );
            assert!(
                description.get("ephemerisData").is_none_or(Value::is_null),
                "case {}: data supplied from an Amazon S3 object carries no inline \
                 ephemerisData, got {description}",
                row.case
            );
        }
        ExpectedData::Verbatim(expected) => {
            assert!(
                description
                    .get("sourceS3Object")
                    .is_none_or(Value::is_null),
                "case {}: data supplied inline names no source Amazon S3 object, got \
                 {description}",
                row.case
            );
            assert_eq!(
                text(description, "ephemerisData"),
                *expected,
                "case {}: wrong described ephemerisData, got {description}",
                row.case
            );
        }
        ExpectedData::AzElJson {
            angle_unit,
            ranges,
            points_per_segment,
        } => {
            assert!(
                description
                    .get("sourceS3Object")
                    .is_none_or(Value::is_null),
                "case {}: data supplied inline names no source Amazon S3 object, got \
                 {description}",
                row.case
            );
            let rendered = text(description, "ephemerisData");
            let document: Value = serde_json::from_str(&rendered).unwrap_or_else(|error| {
                panic!(
                    "case {}: az/el segments are described as a JSON document, which did \
                     not parse ({error}): {rendered:?}",
                    row.case
                )
            });
            assert_eq!(
                text(&document, "angleUnit"),
                *angle_unit,
                "case {}: wrong described angleUnit, got {document}",
                row.case
            );
            let segments = document["azElSegmentList"]
                .as_array()
                .unwrap_or_else(|| {
                    panic!(
                        "case {}: azElSegmentList must be a list, got {document}",
                        row.case
                    )
                })
                .clone();
            assert_eq!(
                segments.len(),
                ranges.len(),
                "case {}: expected one described segment per supplied segment, got \
                 {document}",
                row.case
            );
            for (segment, (start, end)) in segments.iter().zip(ranges) {
                assert_eq!(
                    text(&segment["validTimeRange"], "startTime"),
                    iso8601(*start),
                    "case {}: wrong described segment startTime, got {segment}",
                    row.case
                );
                assert_eq!(
                    text(&segment["validTimeRange"], "endTime"),
                    iso8601(*end),
                    "case {}: wrong described segment endTime, got {segment}",
                    row.case
                );
                assert_eq!(
                    text(segment, "referenceEpoch"),
                    iso8601(*start),
                    "case {}: wrong described segment referenceEpoch, got {segment}",
                    row.case
                );
                assert_eq!(
                    segment["azElList"].as_array().map(Vec::len),
                    Some(*points_per_segment),
                    "case {}: expected one described point per supplied point, got \
                     {segment}",
                    row.case
                );
            }
        }
    }
}

/// `at`, epoch seconds, in the ISO-8601 UTC form the az/el timestamp members are modelled
/// in (`@timestampFormat("date-time")` on `AzElSegment$referenceEpoch` and
/// `ISO8601TimeRange`'s two members).
fn iso8601(at: i64) -> String {
    aws_smithy_types::DateTime::from_secs(at)
        .fmt(aws_smithy_types::date_time::Format::DateTime)
        .expect("a second of the epoch renders in ISO-8601")
}

#[tokio::test]
async fn describe_ephemeris_reports_every_member_of_the_ephemeris_that_was_created() {
    let (router, engine) = harness_with_engine();
    let satellites = onboard(&engine, "25994=Ohio 1,27424");
    let rows = rows();

    let before = now_seconds();
    let mut created: Vec<(String, String, Option<String>, &Row)> = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let name = format!("ephemeris {index}:{}", row.member);
        let mut body = json!({
            "name": name,
            "expirationTime": FUTURE,
            "ephemeris": row.data,
        });
        let satellite = row
            .satellite
            .then(|| satellites[index % satellites.len()].clone());
        if let Some(satellite) = &satellite {
            body["satelliteId"] = json!(satellite);
        }
        if let Some(priority) = row.priority {
            body["priority"] = json!(priority);
        }
        if let Some(enabled) = row.enabled {
            body["enabled"] = json!(enabled);
        }
        if row.tags.as_object().is_some_and(|tags| !tags.is_empty()) {
            body["tags"] = row.tags.clone();
        }
        let id = create_ok(&router, row.case, body).await;
        created.push((id, name, satellite, row));
    }
    let after = now_seconds();

    // Read straight after the create: every member comes back as it was supplied, and the
    // ephemeris is still in the status it was admitted in, because the validation span is
    // never zero.
    let mut first_reads: Vec<Value> = Vec::new();
    for (id, name, satellite, row) in &created {
        let body = describe_ok(&router, row.case, id).await;

        assert_eq!(
            text(&body, "ephemerisId"),
            *id,
            "case {}: an ephemeris is described under the id it was minted with, got {body}",
            row.case
        );
        assert_eq!(
            text(&body, "name"),
            *name,
            "case {}: wrong reported name, got {body}",
            row.case
        );
        assert_eq!(
            body["priority"].as_i64(),
            Some(i64::from(
                row.priority.unwrap_or(DEFAULT_EPHEMERIS_PRIORITY)
            )),
            "case {}: a request that names no priority takes the documented default \
             {DEFAULT_EPHEMERIS_PRIORITY}, got {body}",
            row.case
        );
        assert_eq!(
            body["enabled"].as_bool(),
            Some(row.enabled.unwrap_or(EPHEMERIS_ENABLED_BY_DEFAULT)),
            "case {}: wrong reported enabled, got {body}",
            row.case
        );
        match satellite {
            Some(expected) => assert_eq!(
                text(&body, "satelliteId"),
                *expected,
                "case {}: an ephemeris reports the satellite it was created for, got {body}",
                row.case
            ),
            None => assert!(
                body.get("satelliteId").is_none_or(Value::is_null),
                "case {}: an ephemeris created for no satellite reports none, got {body}",
                row.case
            ),
        }
        assert_eq!(
            body.get("tags"),
            Some(&row.tags),
            "case {}: an ephemeris reports the tags it was created with, got {body}",
            row.case
        );
        let creation_time = body["creationTime"].as_f64().unwrap_or_else(|| {
            panic!(
                "case {}: creationTime must be an epoch-second Timestamp, got {body}",
                row.case
            )
        });
        assert!(
            (before - 1.0..=after + 1.0).contains(&creation_time),
            "case {}: creationTime must be the instant of the create ({before}..{after}), \
             got {creation_time}",
            row.case
        );
        assert_eq!(
            text(&body, "status"),
            EPHEMERIS_VALIDATING,
            "case {}: an ephemeris is admitted in {EPHEMERIS_VALIDATING}, got {body}",
            row.case
        );
        assert!(
            body.get("invalidReason").is_none_or(Value::is_null),
            "case {}: an ephemeris that has not failed validation names no invalidReason, \
             got {body}",
            row.case
        );
        assert!(
            body.get("errorReasons").is_none_or(Value::is_null),
            "case {}: an ephemeris that has not failed validation reports no errorReasons, \
             got {body}",
            row.case
        );
        assert_supplied_data(row, &body["suppliedData"]);

        // A read reports; it changes nothing a later caller sees.
        assert_eq!(
            describe_ok(&router, row.case, id).await,
            body,
            "case {}: describing an ephemeris twice must answer the same ephemeris",
            row.case
        );
        first_reads.push(body);
    }

    // Once the validation the create started falls due, the status is the one the request
    // earned — and nothing else about the ephemeris moved with it.
    wait_out_validation();
    for ((id, _, _, row), first) in created.iter().zip(&first_reads) {
        let settled = describe_ok(&router, row.case, id).await;
        let expected_status = if row.enabled.unwrap_or(EPHEMERIS_ENABLED_BY_DEFAULT) {
            EPHEMERIS_ENABLED
        } else {
            EPHEMERIS_DISABLED
        };
        assert_eq!(
            text(&settled, "status"),
            expected_status,
            "case {}: expected {expected_status} once the validation fell due, got {settled}",
            row.case
        );

        let mut before_settle = first.clone();
        let mut after_settle = settled.clone();
        for body in [&mut before_settle, &mut after_settle] {
            body.as_object_mut()
                .expect("a described ephemeris is an object")
                .remove("status");
        }
        assert_eq!(
            before_settle, after_settle,
            "case {}: only the status moves when a validation falls due",
            row.case
        );
    }
}

/// One row of the status table: what the request asked for, the `EphemerisStatus` the
/// ephemeris must be reported in once the validation the create started has fallen due,
/// and the `EphemerisInvalidReason` it must name then (`None` where it must name none).
struct StatusRow {
    case: &'static str,
    body: Value,
    settled_status: &'static str,
    settled_invalid_reason: Option<&'static str>,
}

#[tokio::test]
async fn describe_ephemeris_names_an_invalid_reason_only_while_the_status_is_invalid() {
    let (router, _engine) = harness_with_engine();

    // Every `EphemerisStatus` this service puts a customer ephemeris in
    // (`smithy_query(query="shape", shape="EphemerisStatus")` — VALIDATING, INVALID, ERROR,
    // ENABLED, DISABLED, EXPIRED). `ERROR` is the service's own processing failure, which
    // nothing in this emulator suffers, so no row claims it.
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
            settled_invalid_reason: None,
        },
        StatusRow {
            case: "asked to stay disabled",
            body: json!({
                "name": "disabled ephemeris",
                "enabled": false,
                "expirationTime": FUTURE,
                "ephemeris": json!({"oem": {"oemData": OEM_BODY}}),
            }),
            settled_status: EPHEMERIS_DISABLED,
            settled_invalid_reason: None,
        },
        StatusRow {
            case: "asked for neither",
            body: json!({
                "name": "default ephemeris",
                "expirationTime": FUTURE,
                "ephemeris": tle_inline(&[(1_800_000_000, 1_800_086_400)]),
            }),
            settled_status: EPHEMERIS_DISABLED,
            settled_invalid_reason: None,
        },
        StatusRow {
            case: "a tle validity range that runs backwards",
            body: json!({
                "name": "backwards tle",
                "enabled": true,
                "expirationTime": FUTURE,
                "ephemeris": tle_inline(&[(1_800_086_400, 1_800_000_000)]),
            }),
            settled_status: EPHEMERIS_INVALID,
            settled_invalid_reason: Some(EPHEMERIS_TIME_RANGE_INVALID),
        },
        StatusRow {
            case: "a tle validity range that ends where it starts",
            body: json!({
                "name": "zero-length tle",
                "enabled": true,
                "expirationTime": FUTURE,
                "ephemeris": tle_inline(&[(1_800_000_000, 1_800_000_000)]),
            }),
            settled_status: EPHEMERIS_INVALID,
            settled_invalid_reason: Some(EPHEMERIS_TIME_RANGE_INVALID),
        },
        StatusRow {
            case: "an azEl segment that runs backwards",
            body: json!({
                "name": "backwards azEl",
                "enabled": false,
                "expirationTime": FUTURE,
                "ephemeris": az_el_inline(&[(1_800_086_400, 1_800_000_000)]),
            }),
            settled_status: EPHEMERIS_INVALID,
            settled_invalid_reason: Some(EPHEMERIS_TIME_RANGE_INVALID),
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
            settled_invalid_reason: None,
        },
    ];

    let mut created: Vec<(String, &StatusRow)> = Vec::new();
    for row in &rows {
        let id = create_ok(&router, row.case, row.body.clone()).await;
        created.push((id, row));
    }

    // While the validation is still owed, EVERY row is `VALIDATING` and names no reason —
    // including the rows whose data has already earned `INVALID`, because the reason
    // "appears only when the status is INVALID".
    for (id, row) in &created {
        let body = describe_ok(&router, row.case, id).await;
        assert_eq!(
            text(&body, "status"),
            EPHEMERIS_VALIDATING,
            "case {}: an ephemeris is admitted in {EPHEMERIS_VALIDATING}, got {body}",
            row.case
        );
        assert!(
            body.get("invalidReason").is_none_or(Value::is_null),
            "case {}: an ephemeris still validating names no invalidReason, got {body}",
            row.case
        );
    }

    wait_out_validation();
    for (id, row) in &created {
        let body = describe_ok(&router, row.case, id).await;
        assert_eq!(
            text(&body, "status"),
            row.settled_status,
            "case {}: expected {} once the validation fell due, got {body}",
            row.case,
            row.settled_status
        );
        match row.settled_invalid_reason {
            Some(reason) => assert_eq!(
                text(&body, "invalidReason"),
                reason,
                "case {}: expected invalidReason {reason}, got {body}",
                row.case
            ),
            None => assert!(
                body.get("invalidReason").is_none_or(Value::is_null),
                "case {}: an ephemeris that is not {EPHEMERIS_INVALID} names no \
                 invalidReason, got {body}",
                row.case
            ),
        }

        // And the verdict holds: reading again reports the same ephemeris.
        assert_eq!(
            describe_ok(&router, row.case, id).await,
            body,
            "case {}: a settled ephemeris must not move again on the next read",
            row.case
        );
    }
}

#[tokio::test]
async fn describe_ephemeris_refuses_an_id_that_names_no_ephemeris_and_stores_nothing() {
    let (router, engine) = harness_with_engine();

    // Well-formed `Uuid`s (the modelled pattern) that name nothing: a run of zeros, a run
    // of `f`s and a mixed one, so the refusal cannot be passing on one id's shape.
    let absent: &[&str] = &[
        ABSENT_UUID,
        "ffffffff-ffff-4fff-bfff-ffffffffffff",
        "0a1b2c3d-4e5f-4a6b-8c7d-9e0f1a2b3c4d",
    ];
    for id in absent {
        let reply = describe(&router, id).await;
        assert_eq!(
            reply.status, 434,
            "case {id}: expected the modelled ResourceNotFoundException status 434, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "ResourceNotFoundException",
            "case {id}: wrong error shape, got {}",
            reply.body
        );
        assert_eq!(
            reply.string("message"),
            EPHEMERIS_NOT_FOUND,
            "case {id}: wrong refusal text, got {}",
            reply.body
        );
    }

    // An id outside the modelled `Uuid` (len 36..36, lower-case hex in five groups) never
    // reaches the handler: this service's request validator answers it, and it is a bad
    // request rather than a missing resource.
    let malformed: &[(&str, &str)] = &[
        ("not hexadecimal at all", "nope"),
        ("one character short", "0000000-0000-0000-0000-000000000000"),
        ("one character long", "000000000-0000-0000-0000-000000000000"),
        ("upper-case hex", "0A1B2C3D-4E5F-4A6B-8C7D-9E0F1A2B3C4D"),
    ];
    for (case, id) in malformed {
        let reply = describe(&router, id).await;
        assert_eq!(
            reply.status, 400,
            "case {case}: expected 400, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            BAD_REQUEST_ERROR_CODE,
            "case {case}: expected {BAD_REQUEST_ERROR_CODE}, got {}",
            reply.body
        );
        assert_eq!(
            reply.string("message"),
            INVALID_REQUEST_BODY,
            "case {case}: wrong refusal text, got {}",
            reply.body
        );
    }

    // Neither refusal disturbed the store: an ephemeris created after them is the only one
    // this account holds, and it still describes.
    let id = create_ok(
        &router,
        "after the refusals",
        json!({
            "name": "surviving ephemeris",
            "enabled": true,
            "expirationTime": FUTURE,
            "ephemeris": tle_inline(&[(1_800_000_000, 1_800_086_400)]),
        }),
    )
    .await;
    let body = describe_ok(&router, "after the refusals", &id).await;
    assert_eq!(
        text(&body, "ephemerisId"),
        id,
        "case after-the-refusals: the created ephemeris must describe, got {body}"
    );
    assert_eq!(
        service_com_amazonaws_groundstation_groundstation::state::service_state(&*engine)
            .expect("this service's state store")
            .lock()
            .ephemerides
            .len(),
        1,
        "case after-the-refusals: a refused describe stores nothing"
    );

    // And a refusal after that create still names nothing that exists.
    let reply = describe(&router, ABSENT_UUID).await;
    assert_eq!(
        reply.status, 434,
        "case absent-beside-a-stored-one: expected 434, got {} {}",
        reply.status, reply.body
    );
}
