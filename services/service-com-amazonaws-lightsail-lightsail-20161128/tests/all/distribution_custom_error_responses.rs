// SPDX-License-Identifier: CC0-1.0

//! The custom error responses a Lightsail content delivery network (CDN)
//! distribution serves, driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rule under test: a member of a nested structure the request carries whose
//! model documents a default is filled in BY THE SERVICE when the entry leaves it
//! out, so the record — and every later read of it — reports that default, while
//! an entry that states a value of its own keeps it and a member the model
//! documents no default for stays absent. For
//! `DistributionCustomErrorResponse` that member is `errorCachingMinTTL`, whose
//! documentation states "If you don't specify a value, the default is `10`
//! seconds"; the other three members are reported as the request stated them.
//!
//! So the cases are a TABLE whose rows differ along what the rule generalises
//! over: whether the entry carries the member at all, the value it carries (both
//! published-looking extremes and zero), how many entries the list holds, which
//! of the entries omit the member, which OTHER members the entry leaves out, and
//! whether the list arrives on the create or on a later update. The bound on the
//! list itself is exercised at the largest list one distribution holds and one
//! entry past it.

use serde_json::{json, Value};

use crate::disk_attachment::create_running_instance;
use crate::distribution_certificate_attachment::{distribution, wait_for_distribution_status};
use crate::instance_lifecycle::{error_code, Emulator};

/// The one Region a distribution lives in and its API is served from, from the
/// modelled `LightsailDistribution$location` documentation ("all distributions
/// are located in the `us-east-1` Region").
const DISTRIBUTION_REGION: &str = "us-east-1";

/// The instance every distribution in this suite pulls content from: the origin
/// is not what any case here is about.
const ORIGIN_INSTANCE: &str = "cer-origin";

/// The bundle every distribution here is created on, an id of the service's
/// published CDN plans (`GetDistributionBundles`).
const BUNDLE_ID: &str = "small_1_0";

/// The `errorCachingMinTTL`, in seconds, an entry that states none is cached for.
///
/// SOURCE: the modelled `DistributionCustomErrorResponse$errorCachingMinTTL`
/// documentation — "The minimum time, in seconds, that the distribution caches
/// the custom error response before requesting the object again from the origin.
/// If you don't specify a value, the default is `10` seconds."
const DEFAULT_ERROR_CACHING_MIN_TTL: i64 = 10;

/// The `status` a distribution settles into once the configuration a call stored
/// has rolled out.
const DEPLOYED: &str = "Deployed";

/// The most custom error responses one distribution holds
/// (`crate::state::MAX_CUSTOM_ERROR_RESPONSES_PER_DISTRIBUTION`).
const MAX_CUSTOM_ERROR_RESPONSES: usize = 25;

/// The wire error code (awsJson `__type`) of a refusal of a request the model
/// admits but the service does not.
const INVALID_INPUT: &str = "InvalidInputException";

// ---------------------------------------------------------------------------
// The table
// ---------------------------------------------------------------------------

/// One `customErrorResponses` list, and the entries the rule says every read of
/// the distribution then reports.
struct Row {
    case: &'static str,
    name: &'static str,
    /// The list as sent, or `None` for a request that carries no
    /// `customErrorResponses` member at all.
    sent: fn() -> Option<Value>,
    /// The entries a read has to report, each carrying exactly the members the
    /// rule says the record holds.
    expected: fn() -> Value,
}

/// The rows.
const ROWS: &[Row] = &[
    Row {
        case: "one entry that omits the member: the documented default is recorded",
        name: "cer-omitted",
        sent: || {
            Some(json!([
                {"errorCode": 404, "responseCode": "404", "responsePagePath": "/404.html"}
            ]))
        },
        expected: || {
            json!([{
                "errorCode": 404,
                "responseCode": "404",
                "responsePagePath": "/404.html",
                "errorCachingMinTTL": DEFAULT_ERROR_CACHING_MIN_TTL,
            }])
        },
    },
    Row {
        case: "one entry that states the member: the value the request asked for is kept",
        name: "cer-stated",
        sent: || {
            Some(json!([{
                "errorCode": 403,
                "responseCode": "403",
                "responsePagePath": "/denied.html",
                "errorCachingMinTTL": 300,
            }]))
        },
        expected: || {
            json!([{
                "errorCode": 403,
                "responseCode": "403",
                "responsePagePath": "/denied.html",
                "errorCachingMinTTL": 300,
            }])
        },
    },
    Row {
        case: "an entry that states zero: an explicit value is not replaced by the default",
        name: "cer-zero",
        sent: || {
            Some(json!([{
                "errorCode": 500,
                "responseCode": "502",
                "responsePagePath": "/oops.html",
                "errorCachingMinTTL": 0,
            }]))
        },
        expected: || {
            json!([{
                "errorCode": 500,
                "responseCode": "502",
                "responsePagePath": "/oops.html",
                "errorCachingMinTTL": 0,
            }])
        },
    },
    Row {
        case: "an entry that states a year of caching: a large value is kept as asked for",
        name: "cer-year",
        sent: || {
            Some(json!([{
                "errorCode": 502,
                "responseCode": "502",
                "responsePagePath": "/gateway.html",
                "errorCachingMinTTL": 31_536_000_i64,
            }]))
        },
        expected: || {
            json!([{
                "errorCode": 502,
                "responseCode": "502",
                "responsePagePath": "/gateway.html",
                "errorCachingMinTTL": 31_536_000_i64,
            }])
        },
    },
    Row {
        case: "three entries, the middle one omitting the member: each entry is filled on its own",
        name: "cer-mixed",
        sent: || {
            Some(json!([
                {
                    "errorCode": 400,
                    "responseCode": "400",
                    "responsePagePath": "/bad.html",
                    "errorCachingMinTTL": 1,
                },
                {"errorCode": 404, "responseCode": "404", "responsePagePath": "/404.html"},
                {
                    "errorCode": 503,
                    "responseCode": "503",
                    "responsePagePath": "/busy.html",
                    "errorCachingMinTTL": 60,
                },
            ]))
        },
        expected: || {
            json!([
                {
                    "errorCode": 400,
                    "responseCode": "400",
                    "responsePagePath": "/bad.html",
                    "errorCachingMinTTL": 1,
                },
                {
                    "errorCode": 404,
                    "responseCode": "404",
                    "responsePagePath": "/404.html",
                    "errorCachingMinTTL": DEFAULT_ERROR_CACHING_MIN_TTL,
                },
                {
                    "errorCode": 503,
                    "responseCode": "503",
                    "responsePagePath": "/busy.html",
                    "errorCachingMinTTL": 60,
                },
            ])
        },
    },
    Row {
        case: "an entry that carries the error code alone: only the member with a documented \
               default is filled in",
        name: "cer-code-only",
        sent: || Some(json!([{"errorCode": 404}])),
        expected: || {
            json!([{"errorCode": 404, "errorCachingMinTTL": DEFAULT_ERROR_CACHING_MIN_TTL}])
        },
    },
    Row {
        case: "an empty entry: the member with a documented default is still filled in",
        name: "cer-empty-entry",
        sent: || Some(json!([{}])),
        expected: || json!([{"errorCachingMinTTL": DEFAULT_ERROR_CACHING_MIN_TTL}]),
    },
    Row {
        case: "an empty list: nothing is invented for a distribution that serves no custom \
               error response",
        name: "cer-empty-list",
        sent: || Some(json!([])),
        expected: || json!([]),
    },
    Row {
        case: "no customErrorResponses member at all: the distribution serves none",
        name: "cer-absent",
        sent: || None,
        expected: || json!([]),
    },
];

/// One `UpdateDistribution` request carrying a replacement list, and the entries
/// the rule says every read of the distribution then reports.
///
/// The rows differ along the same dimensions the create table covers: the member
/// omitted, stated, stated as zero on one entry of several, and a list emptied
/// altogether.
struct UpdateRow {
    case: &'static str,
    /// The `customErrorResponses` the update sends, which replaces the whole
    /// stored list.
    sent: fn() -> Value,
    /// The entries a read has to report once the update has been applied.
    expected: fn() -> Value,
}

/// The update rows.
const UPDATE_ROWS: &[UpdateRow] = &[
    UpdateRow {
        case: "an update that omits the member: the documented default replaces the stored value",
        sent: || {
            json!([
                {"errorCode": 404, "responseCode": "404", "responsePagePath": "/gone.html"}
            ])
        },
        expected: || {
            json!([{
                "errorCode": 404,
                "responseCode": "404",
                "responsePagePath": "/gone.html",
                "errorCachingMinTTL": DEFAULT_ERROR_CACHING_MIN_TTL,
            }])
        },
    },
    UpdateRow {
        case: "an update that states the member: the value the request asked for is kept",
        sent: || {
            json!([{
                "errorCode": 404,
                "responseCode": "404",
                "responsePagePath": "/gone.html",
                "errorCachingMinTTL": 45,
            }])
        },
        expected: || {
            json!([{
                "errorCode": 404,
                "responseCode": "404",
                "responsePagePath": "/gone.html",
                "errorCachingMinTTL": 45,
            }])
        },
    },
    UpdateRow {
        case: "an update that states zero on one entry and omits it on another",
        sent: || {
            json!([
                {
                    "errorCode": 500,
                    "responseCode": "500",
                    "responsePagePath": "/oops.html",
                    "errorCachingMinTTL": 0,
                },
                {"errorCode": 502, "responseCode": "502", "responsePagePath": "/gw.html"},
            ])
        },
        expected: || {
            json!([
                {
                    "errorCode": 500,
                    "responseCode": "500",
                    "responsePagePath": "/oops.html",
                    "errorCachingMinTTL": 0,
                },
                {
                    "errorCode": 502,
                    "responseCode": "502",
                    "responsePagePath": "/gw.html",
                    "errorCachingMinTTL": DEFAULT_ERROR_CACHING_MIN_TTL,
                },
            ])
        },
    },
    UpdateRow {
        case: "an update that empties the list: nothing is invented where there is no entry",
        sent: || json!([]),
        expected: || json!([]),
    },
];

/// Every entry of the largest list one distribution holds, every one of them
/// omitting the member: the fill is per entry, at the list's published bound.
fn full_list() -> Vec<Value> {
    (0..MAX_CUSTOM_ERROR_RESPONSES)
        .map(|index| {
            json!({
                "errorCode": 400 + index as i64,
                "responseCode": format!("{}", 400 + index),
                "responsePagePath": format!("/error-{index}.html"),
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Stand up the emulator with the one origin every case pulls from.
async fn stand_up() -> Emulator {
    let emulator = Emulator::new();
    create_running_instance(
        &emulator,
        ORIGIN_INSTANCE,
        &format!("{DISTRIBUTION_REGION}a"),
    )
    .await;
    emulator
}

/// A `CreateDistribution` body for `name`, carrying `custom_error_responses` when
/// the row sends the member.
fn create_body(name: &str, custom_error_responses: Option<Value>) -> Value {
    let mut body = json!({
        "distributionName": name,
        "bundleId": BUNDLE_ID,
        "origin": {"name": ORIGIN_INSTANCE, "regionName": DISTRIBUTION_REGION},
        "defaultCacheBehavior": {"behavior": "cache"},
    });
    if let Some(responses) = custom_error_responses {
        body["customErrorResponses"] = responses;
    }
    body
}

/// The `customErrorResponses` of `record`, with every member a response left out
/// dropped rather than reported as a null, so a comparison states exactly which
/// members the record holds.
fn reported(record: &Value) -> Value {
    let entries = record["customErrorResponses"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    Value::Array(
        entries
            .iter()
            .map(|entry| match entry.as_object() {
                None => entry.clone(),
                Some(members) => Value::Object(
                    members
                        .iter()
                        .filter(|(_, value)| !value.is_null())
                        .map(|(member, value)| (member.clone(), value.clone()))
                        .collect(),
                ),
            })
            .collect(),
    )
}

// ---------------------------------------------------------------------------
// The cases
// ---------------------------------------------------------------------------

/// `CreateDistribution` records every documented default of a custom error
/// response, and its own answer and a later `GetDistributions` both report it.
#[tokio::test]
async fn create_distribution_fills_the_documented_custom_error_response_default() {
    let emulator = stand_up().await;

    for row in ROWS {
        let case = row.case;
        let created = emulator
            .ok("CreateDistribution", create_body(row.name, (row.sent)()))
            .await;
        let answered = reported(&created["distribution"]);
        let want = (row.expected)();
        assert_eq!(
            answered, want,
            "case {case:?}: expected the create to answer {want:?}, got {answered:?}"
        );

        let read_back = reported(&distribution(&emulator, row.name).await);
        assert_eq!(
            read_back, want,
            "case {case:?}: expected a read of the distribution to report {want:?}, got \
             {read_back:?}"
        );
    }
}

/// The fill is per entry, so the largest list one distribution holds comes back
/// with the documented default on every one of its entries.
#[tokio::test]
async fn every_entry_of_a_full_list_is_filled_in() {
    let emulator = stand_up().await;
    let case = "the largest list one distribution holds, every entry omitting the member";
    let sent = full_list();

    let created = emulator
        .ok(
            "CreateDistribution",
            create_body("cer-full", Some(Value::Array(sent.clone()))),
        )
        .await;
    let answered = reported(&created["distribution"]);
    let entries = answered.as_array().cloned().unwrap_or_default();
    assert_eq!(
        entries.len(),
        sent.len(),
        "case {case:?}: expected {} entries, got {answered:?}",
        sent.len()
    );
    for (index, entry) in entries.iter().enumerate() {
        let ttl = entry["errorCachingMinTTL"].as_i64();
        assert_eq!(
            ttl,
            Some(DEFAULT_ERROR_CACHING_MIN_TTL),
            "case {case:?}: expected entry {index} to carry \
             {DEFAULT_ERROR_CACHING_MIN_TTL}, got {entry:?}"
        );
        let want_path = format!("/error-{index}.html");
        let path = entry["responsePagePath"].as_str();
        assert_eq!(
            path,
            Some(want_path.as_str()),
            "case {case:?}: expected entry {index} to keep the path it was sent with, got \
             {entry:?}"
        );
    }
}

/// A list that arrives on an UPDATE is recorded by the same rule: the entries the
/// request carries replace the stored ones, each with the documented default
/// filled in where the entry left it out.
#[tokio::test]
async fn update_distribution_fills_the_documented_custom_error_response_default() {
    let emulator = stand_up().await;
    let name = "cer-updated";
    emulator
        .ok(
            "CreateDistribution",
            create_body(
                name,
                Some(json!([{
                    "errorCode": 404,
                    "responseCode": "404",
                    "responsePagePath": "/404.html",
                    "errorCachingMinTTL": 600,
                }])),
            ),
        )
        .await;
    wait_for_distribution_status(&emulator, name, DEPLOYED).await;

    // Each update replaces the whole list, applied in order to the one
    // distribution, so each row also checks the row before it left nothing behind.
    for row in UPDATE_ROWS {
        let case = row.case;
        emulator
            .ok(
                "UpdateDistribution",
                json!({
                    "distributionName": name,
                    "customErrorResponses": (row.sent)(),
                }),
            )
            .await;
        let read_back = reported(&distribution(&emulator, name).await);
        let want = (row.expected)();
        assert_eq!(
            read_back, want,
            "case {case:?}: expected the distribution to report {want:?}, got {read_back:?}"
        );
    }

    // A request that names no `customErrorResponses` at all leaves the stored
    // list alone, so the fill never runs on a list nobody sent.
    let kept = json!([{
        "errorCode": 404,
        "responseCode": "404",
        "responsePagePath": "/404.html",
        "errorCachingMinTTL": DEFAULT_ERROR_CACHING_MIN_TTL,
    }]);
    emulator
        .ok(
            "UpdateDistribution",
            json!({"distributionName": name, "customErrorResponses": [
                {"errorCode": 404, "responseCode": "404", "responsePagePath": "/404.html"}
            ]}),
        )
        .await;
    emulator
        .ok(
            "UpdateDistribution",
            json!({"distributionName": name, "isEnabled": false}),
        )
        .await;
    let read_back = reported(&distribution(&emulator, name).await);
    assert_eq!(
        read_back, kept,
        "case {:?}: expected the stored list to survive untouched, got {read_back:?}",
        "an update that names no customErrorResponses leaves the list alone"
    );
}

/// A list longer than one distribution holds is refused, by both operations, and
/// changes nothing — the bound the fill is applied behind.
#[tokio::test]
async fn a_list_past_the_bound_is_refused_by_both_operations() {
    let emulator = stand_up().await;
    let mut too_many = full_list();
    too_many.push(json!({"errorCode": 599, "responseCode": "599"}));

    let case = "a create carrying one entry more than a distribution holds";
    let (status, body) = emulator
        .call(
            "CreateDistribution",
            create_body("cer-too-many", Some(Value::Array(too_many.clone()))),
        )
        .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        INVALID_INPUT,
        "case {case:?}: expected {INVALID_INPUT}, got {body:?}"
    );
    let message = body["message"].as_str().unwrap_or_default().to_string();
    assert!(
        message.contains(&too_many.len().to_string())
            && message.contains(&MAX_CUSTOM_ERROR_RESPONSES.to_string()),
        "case {case:?}: expected the refusal to name {} entries and the {MAX_CUSTOM_ERROR_RESPONSES} \
         a distribution holds, got {message:?}",
        too_many.len()
    );
    let listed = emulator.ok("GetDistributions", json!({})).await;
    let names: Vec<&str> = listed["distributions"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| entry["name"].as_str())
                .collect()
        })
        .unwrap_or_default();
    assert!(
        !names.contains(&"cer-too-many"),
        "case {case:?}: expected the refused create to store nothing, got {names:?}"
    );

    let case = "an update carrying one entry more than a distribution holds";
    let name = "cer-bound";
    emulator
        .ok(
            "CreateDistribution",
            create_body(
                name,
                Some(json!([
                    {"errorCode": 404, "responseCode": "404", "responsePagePath": "/404.html"}
                ])),
            ),
        )
        .await;
    let (status, body) = emulator
        .call(
            "UpdateDistribution",
            json!({
                "distributionName": name,
                "customErrorResponses": Value::Array(too_many.clone()),
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        INVALID_INPUT,
        "case {case:?}: expected {INVALID_INPUT}, got {body:?}"
    );
    let read_back = reported(&distribution(&emulator, name).await);
    let want = json!([{
        "errorCode": 404,
        "responseCode": "404",
        "responsePagePath": "/404.html",
        "errorCachingMinTTL": DEFAULT_ERROR_CACHING_MIN_TTL,
    }]);
    assert_eq!(
        read_back, want,
        "case {case:?}: expected the refused update to change nothing, got {read_back:?}"
    );
}
