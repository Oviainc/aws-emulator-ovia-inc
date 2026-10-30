// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the value a NESTED member of an app monitor's structures
//! settles on when the request that carried the structure left that member out.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with a signed
//! restJson1 request — so routing, deserialization, the modelled constraints, the handler
//! and the response envelope are all exercised, not just a handler body.
//!
//! The rule pinned here: a structure a request carries is stored with every member it left
//! unset settled onto the value the model documents for that member when it is omitted —
//! `AppMonitorConfiguration$SessionSampleRate` on 0.1 ("If you omit this parameter, the
//! default of 0.1 is used, and 10% of sessions will be sampled") and `CustomEvents$Status`
//! on `DISABLED` ("The default is for custom events to be `DISABLED`") — whether the
//! request omitted the whole structure or carried it without that member, and on the create
//! path and the update path alike. A member the model documents no value for when omitted
//! stays exactly as it arrived, absence included: hence the rows that carry a
//! `DeobfuscationConfiguration` naming no source maps, which pin the boundary of the rule
//! (`DeobfuscationConfiguration$JavaScriptSourceMaps` documents no value for an omitted
//! parameter, and the "default is DISABLED" sentence sits on the `@required`
//! `JavaScriptSourceMaps$Status`, which a request naming source maps always carries).
//!
//! The table spans what the rule generalises over: which of the three structures the
//! request carries, which members inside it the request leaves out, the whole
//! `CustomEventsStatus` and `DeobfuscationStatus` catalogs, and the session sample rate at
//! both published bounds of its `@range`, just inside it, and one step past it.

use serde_json::{json, Value};
use service_com_amazonaws_rum_rum::state::{
    DEFAULT_CUSTOM_EVENTS_STATUS, DEFAULT_DEOBFUSCATION_STATUS, DEFAULT_SESSION_SAMPLE_RATE,
};

use crate::support::{call, create, emulator, get_app_monitor, REGION};

/// The structure members of an app monitor a request can carry only part of — the three
/// `CreateAppMonitor` and `UpdateAppMonitor` both accept.
/// Source: `smithy_query(query="operation", operation="CreateAppMonitor,UpdateAppMonitor")`.
const NESTED_STRUCTURES: [&str; 3] = [
    "AppMonitorConfiguration",
    "CustomEvents",
    "DeobfuscationConfiguration",
];

/// Every value `CustomEventsStatus` admits.
/// Source: `smithy_query(query="shape", shape="CustomEventsStatus")`.
const CUSTOM_EVENTS_STATUSES: [&str; 2] = ["DISABLED", "ENABLED"];

/// Every value `DeobfuscationStatus` admits.
/// Source: `smithy_query(query="shape", shape="DeobfuscationStatus")`.
const DEOBFUSCATION_STATUSES: [&str; 2] = ["DISABLED", "ENABLED"];

/// Every value `Telemetry` admits.
/// Source: `smithy_query(query="shape", shape="Telemetry")`.
const TELEMETRIES: [&str; 3] = ["errors", "performance", "http"];

/// The bucket a source-map configuration reads maps out of — required whenever
/// unminification is `ENABLED`.
const MAPS_BUCKET: &str = "s3://maps-bucket/app";

/// One row: the structure members a request carries, and what a read must then report for
/// each of them.
struct Row {
    case: String,
    /// The structure members the request carries, spelled as the request spells them. An
    /// empty object is the request that carries none of them.
    sent: Value,
    /// What a read must report for each member `sent` carries, or `None` when the request
    /// is refused outright and must store nothing.
    settled: Option<Value>,
}

/// The value a read reports for a structure the request named NOWHERE: the documented
/// default for the member a request can leave unset, and — for the deobfuscation
/// configuration, whose nested member documents none — the structure a monitor created
/// without one carries.
/// Source: `smithy_query(query="shape", shape="AppMonitorConfiguration,CustomEvents,JavaScriptSourceMaps")`.
fn documented_default(member: &str) -> Value {
    match member {
        "AppMonitorConfiguration" => json!({"SessionSampleRate": DEFAULT_SESSION_SAMPLE_RATE}),
        "CustomEvents" => json!({"Status": DEFAULT_CUSTOM_EVENTS_STATUS}),
        "DeobfuscationConfiguration" => {
            json!({"JavaScriptSourceMaps": {"Status": DEFAULT_DEOBFUSCATION_STATUS}})
        }
        other => panic!("{other} is not one of this app monitor's nested structures"),
    }
}

/// The structures the update table's monitor is SEEDED with: every member that has a
/// documented default named explicitly, and named something other than that default, so a
/// later partial update that drops the member cannot pass by leaving the seeded value in
/// place.
fn seeded_structures() -> Value {
    json!({
        "AppMonitorConfiguration": {"SessionSampleRate": 0.5, "AllowCookies": true},
        "CustomEvents": {"Status": "ENABLED"},
        "DeobfuscationConfiguration": {
            "JavaScriptSourceMaps": {"Status": "ENABLED", "S3Uri": MAPS_BUCKET}
        },
    })
}

/// The table: one row per way a request can leave a nested member unset, plus the rows that
/// name the member explicitly (which must be stored as sent) and the one that puts the rate
/// past its modelled range (which must be refused).
fn rows() -> Vec<Row> {
    let identity_pool = "us-east-1:0123abcd-0123-4567-89ab-0123456789ab";
    let guest_role = engine::emulator_arn("iam", "", "role/rum-guest");
    let mut rows = vec![
        Row {
            case: "no structure at all".to_string(),
            sent: json!({}),
            settled: Some(json!({})),
        },
        Row {
            case: "AppMonitorConfiguration naming only AllowCookies".to_string(),
            sent: json!({"AppMonitorConfiguration": {"AllowCookies": true}}),
            settled: Some(json!({
                "AppMonitorConfiguration": {
                    "AllowCookies": true,
                    "SessionSampleRate": DEFAULT_SESSION_SAMPLE_RATE,
                }
            })),
        },
        Row {
            case: "AppMonitorConfiguration naming only EnableXRay".to_string(),
            sent: json!({"AppMonitorConfiguration": {"EnableXRay": true}}),
            settled: Some(json!({
                "AppMonitorConfiguration": {
                    "EnableXRay": true,
                    "SessionSampleRate": DEFAULT_SESSION_SAMPLE_RATE,
                }
            })),
        },
        Row {
            case: "AppMonitorConfiguration naming the pages and telemetry it collects"
                .to_string(),
            sent: json!({
                "AppMonitorConfiguration": {
                    "Telemetries": TELEMETRIES,
                    "FavoritePages": ["/home", "/checkout"],
                    "IncludedPages": ["https://example.com/home"],
                }
            }),
            settled: Some(json!({
                "AppMonitorConfiguration": {
                    "Telemetries": TELEMETRIES,
                    "FavoritePages": ["/home", "/checkout"],
                    "IncludedPages": ["https://example.com/home"],
                    "SessionSampleRate": DEFAULT_SESSION_SAMPLE_RATE,
                }
            })),
        },
        Row {
            case: "AppMonitorConfiguration naming only its Cognito authorization".to_string(),
            sent: json!({
                "AppMonitorConfiguration": {
                    "IdentityPoolId": identity_pool,
                    "GuestRoleArn": guest_role,
                }
            }),
            settled: Some(json!({
                "AppMonitorConfiguration": {
                    "IdentityPoolId": identity_pool,
                    "GuestRoleArn": guest_role,
                    "SessionSampleRate": DEFAULT_SESSION_SAMPLE_RATE,
                }
            })),
        },
        Row {
            // The bottom of `SessionSampleRate`'s modelled `@range` is also the member's
            // modelled `@default`, which is the value an omitted rate arrives as — so this
            // row is the omitted rate, and settles on the documented 0.1 like the rest.
            case: "AppMonitorConfiguration naming the rate at the bottom of its range"
                .to_string(),
            sent: json!({"AppMonitorConfiguration": {"SessionSampleRate": 0.0}}),
            settled: Some(json!({
                "AppMonitorConfiguration": {"SessionSampleRate": DEFAULT_SESSION_SAMPLE_RATE}
            })),
        },
        Row {
            case: "AppMonitorConfiguration naming a rate inside its range".to_string(),
            sent: json!({"AppMonitorConfiguration": {"SessionSampleRate": 0.25}}),
            settled: Some(json!({"AppMonitorConfiguration": {"SessionSampleRate": 0.25}})),
        },
        Row {
            case: "AppMonitorConfiguration naming the rate at the top of its range".to_string(),
            sent: json!({"AppMonitorConfiguration": {"SessionSampleRate": 1.0}}),
            settled: Some(json!({"AppMonitorConfiguration": {"SessionSampleRate": 1.0}})),
        },
        Row {
            case: "AppMonitorConfiguration naming a rate past the top of its range".to_string(),
            sent: json!({"AppMonitorConfiguration": {"SessionSampleRate": 1.5}}),
            settled: None,
        },
        Row {
            case: "CustomEvents carrying no Status".to_string(),
            sent: json!({"CustomEvents": {}}),
            settled: Some(json!({"CustomEvents": {"Status": DEFAULT_CUSTOM_EVENTS_STATUS}})),
        },
        Row {
            // The boundary of the rule: this structure's one member documents no value for
            // an omitted parameter, so a configuration that names no source maps is stored
            // as it arrived rather than settled onto anything.
            case: "DeobfuscationConfiguration carrying no JavaScriptSourceMaps".to_string(),
            sent: json!({"DeobfuscationConfiguration": {}}),
            settled: Some(json!({"DeobfuscationConfiguration": {}})),
        },
        Row {
            case: "every structure at once, each missing a member".to_string(),
            sent: json!({
                "AppMonitorConfiguration": {"AllowCookies": false},
                "CustomEvents": {},
                "DeobfuscationConfiguration": {},
            }),
            settled: Some(json!({
                "AppMonitorConfiguration": {
                    "AllowCookies": false,
                    "SessionSampleRate": DEFAULT_SESSION_SAMPLE_RATE,
                },
                "CustomEvents": documented_default("CustomEvents"),
                "DeobfuscationConfiguration": {},
            })),
        },
    ];
    // The whole `CustomEventsStatus` catalog named explicitly: a status the request DID
    // name is stored as it arrived, whichever value it is.
    for status in CUSTOM_EVENTS_STATUSES {
        rows.push(Row {
            case: format!("CustomEvents naming Status {status}"),
            sent: json!({"CustomEvents": {"Status": status}}),
            settled: Some(json!({"CustomEvents": {"Status": status}})),
        });
    }
    // And the whole `DeobfuscationStatus` catalog, each with the bucket its status needs:
    // `S3Uri` "is required if status is ENABLED".
    for status in DEOBFUSCATION_STATUSES {
        let mut source_maps = json!({"Status": status});
        if status == "ENABLED" {
            source_maps["S3Uri"] = json!(MAPS_BUCKET);
        }
        rows.push(Row {
            case: format!("DeobfuscationConfiguration naming source maps {status}"),
            sent: json!({
                "DeobfuscationConfiguration": {"JavaScriptSourceMaps": source_maps.clone()}
            }),
            settled: Some(json!({
                "DeobfuscationConfiguration": {"JavaScriptSourceMaps": source_maps}
            })),
        });
    }
    rows
}

/// `value` with every number read as a double, so a row that spells a rate `1.0` and a
/// body that writes it `1` compare equal: the comparison is about the VALUE, not about
/// which JSON number form carried it.
fn with_f64_numbers(value: &Value) -> Value {
    match value {
        Value::Number(number) => number
            .as_f64()
            .and_then(serde_json::Number::from_f64)
            .map_or(Value::Null, Value::Number),
        Value::Array(entries) => Value::Array(entries.iter().map(with_f64_numbers).collect()),
        Value::Object(members) => Value::Object(
            members
                .iter()
                .map(|(member, nested)| (member.clone(), with_f64_numbers(nested)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// Assert the read reports `member` as `want`.
fn assert_reported(case: &str, monitor: &Value, member: &str, want: &Value) {
    let got = with_f64_numbers(&monitor[member]);
    assert_eq!(
        got,
        with_f64_numbers(want),
        "case {case:?}: expected {member} to be reported as {want}, got {got}"
    );
}

/// The members a row's request did NOT carry — the ones whose stored value the request
/// says nothing about.
fn untouched_structures(sent: &Value) -> impl Iterator<Item = &'static str> + '_ {
    NESTED_STRUCTURES
        .into_iter()
        .filter(move |member| sent.get(*member).is_none())
}

#[tokio::test]
async fn a_create_settles_every_nested_member_it_left_unset_on_its_documented_default() {
    let (router, _engine) = emulator();

    for (index, Row { case, sent, settled }) in rows().into_iter().enumerate() {
        let case = case.as_str();
        let name = format!("created-{index}");
        let mut body = json!({"Name": name, "Domain": "example.com"});
        let object = body.as_object_mut().expect("a JSON object");
        for (member, value) in sent.as_object().expect("a JSON object") {
            object.insert(member.clone(), value.clone());
        }

        let Some(settled) = settled else {
            // The one row outside the member's modelled range: refused, and nothing stored.
            let reply = call(&router, "POST", "/appmonitor", Some(body)).await;
            assert_eq!(
                reply.status, 400,
                "case {case:?}: a value outside the member's modelled range must be refused \
                 with HTTP 400, got {} {:?}",
                reply.status,
                String::from_utf8_lossy(&reply.body)
            );
            assert_eq!(
                reply.error_code(case),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {:?}",
                String::from_utf8_lossy(&reply.body)
            );
            let read = call(&router, "GET", &format!("/appmonitor/{name}"), None).await;
            assert_eq!(
                read.status, 404,
                "case {case:?}: a refused create must store nothing, got {} {:?}",
                read.status,
                String::from_utf8_lossy(&read.body)
            );
            continue;
        };

        create(&router, case, body).await;
        let monitor = get_app_monitor(&router, case, &name).await;
        // The members the request carried: stored as sent, with the member it left unset
        // settled onto the documented default.
        for (member, want) in settled.as_object().expect("a JSON object") {
            assert_reported(case, &monitor, member, want);
        }
        // And every structure the request named at all: materialised on its documented
        // default rather than left off the wire.
        for member in untouched_structures(&sent) {
            assert_reported(case, &monitor, member, &documented_default(member));
        }
    }
}

#[tokio::test]
async fn an_update_settles_the_same_defaults_and_keeps_the_structures_it_names_none_of() {
    // The same table against `UpdateAppMonitor`, over a monitor seeded with a value for
    // every defaulted member: a structure the request carries replaces the stored one and
    // settles the same way, while a structure the request names at all is kept as it was.
    let (router, _engine) = emulator();
    let seeded = seeded_structures();

    for (index, Row { case, sent, settled }) in rows().into_iter().enumerate() {
        let case = case.as_str();
        let name = format!("updated-{index}");
        let mut seed = json!({"Name": name, "Domain": "example.com"});
        let object = seed.as_object_mut().expect("a JSON object");
        for (member, value) in seeded.as_object().expect("a JSON object") {
            object.insert(member.clone(), value.clone());
        }
        create(&router, case, seed).await;

        let reply = call(
            &router,
            "PATCH",
            &format!("/appmonitor/{name}"),
            Some(sent.clone()),
        )
        .await;

        let Some(settled) = settled else {
            assert_eq!(
                reply.status, 400,
                "case {case:?}: a value outside the member's modelled range must be refused \
                 with HTTP 400, got {} {:?}",
                reply.status,
                String::from_utf8_lossy(&reply.body)
            );
            assert_eq!(
                reply.error_code(case),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {:?}",
                String::from_utf8_lossy(&reply.body)
            );
            let monitor = get_app_monitor(&router, case, &name).await;
            for member in NESTED_STRUCTURES {
                assert_reported(case, &monitor, member, &seeded[member]);
            }
            continue;
        };

        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected UpdateAppMonitor to answer 200, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        let monitor = get_app_monitor(&router, case, &name).await;
        for (member, want) in settled.as_object().expect("a JSON object") {
            assert_reported(case, &monitor, member, want);
        }
        // "For any parameters that you omit, the existing values are kept": a structure
        // this request did not name keeps the value the monitor was created with.
        for member in untouched_structures(&sent) {
            assert_reported(case, &monitor, member, &seeded[member]);
        }
    }
}

#[tokio::test]
async fn a_partial_update_never_leaves_a_monitor_sampling_nothing() {
    // The consequence the rule rules out, end to end and in the region every case signs
    // for: a monitor created with a rate of its own, updated by a request that means only
    // to flip `AllowCookies`, reports the documented 10% — never 0, and never a member
    // missing from the read.
    let (router, _engine) = emulator();
    let name = "partial-update";
    create(
        &router,
        "seed",
        json!({
            "Name": name,
            "Domain": "example.com",
            "AppMonitorConfiguration": {"SessionSampleRate": 0.5},
        }),
    )
    .await;

    let reply = call(
        &router,
        "PATCH",
        &format!("/appmonitor/{name}"),
        Some(json!({"AppMonitorConfiguration": {"AllowCookies": true}})),
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "case \"flip AllowCookies\": expected UpdateAppMonitor to answer 200 in {REGION}, got \
         {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );

    let monitor = get_app_monitor(&router, "flip AllowCookies", name).await;
    let rate = monitor["AppMonitorConfiguration"]["SessionSampleRate"].as_f64();
    assert_eq!(
        rate,
        Some(DEFAULT_SESSION_SAMPLE_RATE),
        "case \"flip AllowCookies\": expected the documented default rate \
         {DEFAULT_SESSION_SAMPLE_RATE}, got {rate:?} in {:?}",
        monitor["AppMonitorConfiguration"]
    );
    assert_eq!(
        monitor["AppMonitorConfiguration"]["AllowCookies"].as_bool(),
        Some(true),
        "case \"flip AllowCookies\": expected the member the update named to be stored, got \
         {:?}",
        monitor["AppMonitorConfiguration"]
    );
}
