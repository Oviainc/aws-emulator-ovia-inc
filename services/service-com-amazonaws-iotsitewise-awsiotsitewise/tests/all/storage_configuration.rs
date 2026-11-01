// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the IoT SiteWise storage configuration
//! (`PutStorageConfiguration` / `DescribeStorageConfiguration`), driven over
//! HTTP through the same router the emulator boots with.
//!
//! The rule these tests pin: the storage configuration is one account-wide
//! SETTING, so a put replaces it whole and a describe reports exactly what the
//! last put stored, with the state the account carries filled in for each member
//! the configuration does not name — including both tier-state members, which
//! every answer reports; an account no put has configured reports the hot-tier
//! default without storing anything; the configuration a put accepts
//! is reported as being applied until the span the service settles a transition
//! in has passed, and the describe reports when the last put recorded it; a
//! selection outside the rules the shapes state — a tier outside the modelled
//! enum, a cold-tier destination that does not go with the tier named, a
//! retention period that names no period or two, a warm-tier period where no
//! cold tier keeps a copy — is refused rather than stored; and the one
//! member that does not go back, disassociated data storage, cannot be disabled
//! once a put has enabled it.

use std::time::{Duration, Instant, SystemTime};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::ops::types::{
    CONFIGURATION_STATE_VALUES, DISASSOCIATED_DATA_STORAGE_STATE_VALUES, STORAGE_TYPE_VALUES,
    WARM_TIER_STATE_VALUES,
};
use service_com_amazonaws_iotsitewise_awsiotsitewise::state::{
    ACTIVATION_DELAY, CONFIGURATION_STATE_ACTIVE, CONFIGURATION_STATE_UPDATE_IN_PROGRESS,
    DEFAULT_DISALLOW_INGEST_NULL_NAN, STORAGE_STATE_DISABLED, STORAGE_STATE_ENABLED,
    STORAGE_TYPE_MULTI_LAYER, STORAGE_TYPE_SITEWISE_DEFAULT,
};

use crate::asset_lifecycle::{call, error_code, router};

/// The URI both operations are bound to, from
/// `smithy_query(query="operation", operation="PutStorageConfiguration")`:
/// `POST /configuration/account/storage` and the `GET` of the same path.
const URI: &str = "/configuration/account/storage";

/// Every value the modelled `StorageType` enum accepts, written out here as an
/// oracle independent of the crate under test.
///
/// Source: `smithy_query(query="shape", shape="StorageType")` on this service's
/// model, which reports
/// `values: ["SITEWISE_DEFAULT_STORAGE", "MULTI_LAYER_STORAGE"]`.
const MODELLED_STORAGE_TYPES: [&str; 2] = ["MULTI_LAYER_STORAGE", "SITEWISE_DEFAULT_STORAGE"];

/// Every value the modelled `DisassociatedDataStorageState` and `WarmTierState`
/// enums accept — the same pair for both, likewise an independent oracle.
///
/// Source: `smithy_query(query="shape",
/// shape="DisassociatedDataStorageState,WarmTierState")`, which reports
/// `values: ["ENABLED", "DISABLED"]` for each.
const MODELLED_FEATURE_STATES: [&str; 2] = ["DISABLED", "ENABLED"];

/// A cold-tier destination, whose two members are each a modelled `ARN`.
///
/// This helper, `put`, `describe` and `reported` are `pub(crate)` because the
/// read-side suite `describe_storage_configuration_reads.rs` drives the same two
/// operations through them rather than growing a second copy of the request
/// mechanics.
pub(crate) fn multi_layer_storage(bucket: &str, role: &str) -> Value {
    json!({
        "customerManagedS3Storage": {
            "s3ResourceArn": format!("arn:aws:s3:::{bucket}"),
            "roleArn": format!("arn:aws:iam::000000000000:role/{role}"),
        }
    })
}

/// Put a configuration and return `(status, body)`.
pub(crate) async fn put(router: &MultiServiceRouter, body: Value) -> (u16, Value) {
    call(router, "POST", URI, Some(body)).await
}

/// Describe the account's configuration and return `(status, body)`.
pub(crate) async fn describe(router: &MultiServiceRouter) -> (u16, Value) {
    call(router, "GET", URI, None).await
}

/// The seven members both operations answer the configuration with, as one
/// value: a member the answer omits reads as `null`, which is how the
/// expectations below spell "not reported".
pub(crate) fn reported(body: &Value) -> Value {
    json!({
        "storageType": body["storageType"],
        "multiLayerStorage": body["multiLayerStorage"],
        "disassociatedDataStorage": body["disassociatedDataStorage"],
        "retentionPeriod": body["retentionPeriod"],
        "warmTier": body["warmTier"],
        "warmTierRetentionPeriod": body["warmTierRetentionPeriod"],
        "disallowIngestNullNaN": body["disallowIngestNullNaN"],
    })
}

/// The configuration an account no put has configured reports: the hot tier,
/// data kept indefinitely in it, NULL/NaN ingestion allowed, data streams no
/// asset property owns accepted, and the warm tier — a tier an account opts
/// into — off.
fn unconfigured() -> Value {
    json!({
        "storageType": STORAGE_TYPE_SITEWISE_DEFAULT,
        "multiLayerStorage": Value::Null,
        "disassociatedDataStorage": STORAGE_STATE_ENABLED,
        "retentionPeriod": { "unlimited": true },
        "warmTier": STORAGE_STATE_DISABLED,
        "warmTierRetentionPeriod": Value::Null,
        "disallowIngestNullNaN": DEFAULT_DISALLOW_INGEST_NULL_NAN,
    })
}

/// Seconds since the epoch, for reading `lastUpdateDate` against the clock.
fn now_seconds() -> f64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("a clock after the epoch")
        .as_secs_f64()
}

/// The generated value tables the handlers answer from carry exactly the
/// modelled enums, and the values this service names are drawn from them.
#[test]
fn the_generated_value_tables_are_the_modelled_enums() {
    let mut types = STORAGE_TYPE_VALUES.to_vec();
    types.sort_unstable();
    assert_eq!(
        types, MODELLED_STORAGE_TYPES,
        "the generated StorageType table drifted from the model: expected \
         {MODELLED_STORAGE_TYPES:?}, got {STORAGE_TYPE_VALUES:?}"
    );

    for (name, table) in [
        (
            "DisassociatedDataStorageState",
            DISASSOCIATED_DATA_STORAGE_STATE_VALUES,
        ),
        ("WarmTierState", WARM_TIER_STATE_VALUES),
    ] {
        let mut states = table.to_vec();
        states.sort_unstable();
        assert_eq!(
            states, MODELLED_FEATURE_STATES,
            "case {name:?}: the generated table drifted from the model: expected \
             {MODELLED_FEATURE_STATES:?}, got {table:?}"
        );
    }

    for value in [STORAGE_TYPE_SITEWISE_DEFAULT, STORAGE_TYPE_MULTI_LAYER] {
        assert!(
            MODELLED_STORAGE_TYPES.contains(&value),
            "case {value:?}: the service names a storage type the model does not define \
             ({MODELLED_STORAGE_TYPES:?})"
        );
    }
    for value in [STORAGE_STATE_ENABLED, STORAGE_STATE_DISABLED] {
        assert!(
            MODELLED_FEATURE_STATES.contains(&value),
            "case {value:?}: the service names a feature state the model does not define \
             ({MODELLED_FEATURE_STATES:?})"
        );
    }
}

/// An account no put has configured reports the hot-tier default with no update
/// date, and reading it leaves it that way — a describe is a read, so a later
/// caller sees what the first one saw.
#[tokio::test]
async fn an_unconfigured_account_reports_the_hot_tier_default_and_stays_that_way() {
    let router = router();

    for attempt in 1..=3 {
        let (status, body) = describe(&router).await;
        assert_eq!(
            status, 200,
            "describe attempt {attempt} with nothing configured: got {status} {body}"
        );
        assert_eq!(
            reported(&body),
            unconfigured(),
            "describe attempt {attempt} with nothing configured: got {body}"
        );
        assert_eq!(
            body["configurationStatus"]["state"].as_str(),
            Some(CONFIGURATION_STATE_ACTIVE),
            "describe attempt {attempt} with nothing configured: got {body}"
        );
        assert_eq!(
            body["lastUpdateDate"],
            Value::Null,
            "describe attempt {attempt} with nothing configured reported an update date: \
             got {body}"
        );
    }
}

/// One row of the round-trip table: the body a put sends and the seven members
/// the service must report for it.
struct Case {
    label: &'static str,
    body: Value,
    expected: Value,
}

/// The configurations a put accepts, spanning what the rule generalises over:
/// each storage tier, a cold-tier destination, both forms a retention period
/// takes (a day count — several sizes — and unlimited), the warm tier with and
/// without a period of its own, each feature state, and both values of the
/// NULL/NaN switch. Every row also pins which members the answer fills in for
/// itself: hot-tier retention and the NULL/NaN switch, whose defaults the
/// documentation states.
fn round_trip_cases() -> Vec<Case> {
    vec![
        Case {
            label: "the hot tier alone, naming nothing else",
            body: json!({ "storageType": STORAGE_TYPE_SITEWISE_DEFAULT }),
            expected: unconfigured(),
        },
        Case {
            label: "the hot tier with a one-day retention period",
            body: json!({
                "storageType": STORAGE_TYPE_SITEWISE_DEFAULT,
                "retentionPeriod": { "numberOfDays": 1 },
            }),
            expected: json!({
                "storageType": STORAGE_TYPE_SITEWISE_DEFAULT,
                "multiLayerStorage": Value::Null,
                "disassociatedDataStorage": STORAGE_STATE_ENABLED,
                "retentionPeriod": { "numberOfDays": 1 },
                "warmTier": STORAGE_STATE_DISABLED,
                "warmTierRetentionPeriod": Value::Null,
                "disallowIngestNullNaN": false,
            }),
        },
        Case {
            label: "the hot tier with a 3650-day retention period and NULL/NaN disallowed",
            body: json!({
                "storageType": STORAGE_TYPE_SITEWISE_DEFAULT,
                "retentionPeriod": { "numberOfDays": 3650, "unlimited": false },
                "disallowIngestNullNaN": true,
            }),
            expected: json!({
                "storageType": STORAGE_TYPE_SITEWISE_DEFAULT,
                "multiLayerStorage": Value::Null,
                "disassociatedDataStorage": STORAGE_STATE_ENABLED,
                "retentionPeriod": { "numberOfDays": 3650, "unlimited": false },
                "warmTier": STORAGE_STATE_DISABLED,
                "warmTierRetentionPeriod": Value::Null,
                "disallowIngestNullNaN": true,
            }),
        },
        Case {
            label: "the hot tier with data kept indefinitely and the two features off",
            body: json!({
                "storageType": STORAGE_TYPE_SITEWISE_DEFAULT,
                "retentionPeriod": { "unlimited": true },
                "disassociatedDataStorage": STORAGE_STATE_DISABLED,
                "warmTier": STORAGE_STATE_DISABLED,
            }),
            expected: json!({
                "storageType": STORAGE_TYPE_SITEWISE_DEFAULT,
                "multiLayerStorage": Value::Null,
                "disassociatedDataStorage": STORAGE_STATE_DISABLED,
                "retentionPeriod": { "unlimited": true },
                "warmTier": STORAGE_STATE_DISABLED,
                "warmTierRetentionPeriod": Value::Null,
                "disallowIngestNullNaN": false,
            }),
        },
        Case {
            label: "the cold tier, with a destination and an enabled warm tier",
            body: json!({
                "storageType": STORAGE_TYPE_MULTI_LAYER,
                "multiLayerStorage": multi_layer_storage("plant-1-archive", "SiteWiseColdTier"),
                "warmTier": STORAGE_STATE_ENABLED,
            }),
            expected: json!({
                "storageType": STORAGE_TYPE_MULTI_LAYER,
                "multiLayerStorage": multi_layer_storage("plant-1-archive", "SiteWiseColdTier"),
                "disassociatedDataStorage": STORAGE_STATE_ENABLED,
                "retentionPeriod": { "unlimited": true },
                "warmTier": STORAGE_STATE_ENABLED,
                "warmTierRetentionPeriod": Value::Null,
                "disallowIngestNullNaN": false,
            }),
        },
        Case {
            label: "the cold tier, with a warm tier that keeps 90 days",
            body: json!({
                "storageType": STORAGE_TYPE_MULTI_LAYER,
                "multiLayerStorage": multi_layer_storage("plant-2-archive/prefix", "ColdRole_2"),
                "retentionPeriod": { "numberOfDays": 30 },
                "warmTier": STORAGE_STATE_ENABLED,
                "warmTierRetentionPeriod": { "numberOfDays": 90 },
            }),
            expected: json!({
                "storageType": STORAGE_TYPE_MULTI_LAYER,
                "multiLayerStorage": multi_layer_storage("plant-2-archive/prefix", "ColdRole_2"),
                "disassociatedDataStorage": STORAGE_STATE_ENABLED,
                "retentionPeriod": { "numberOfDays": 30 },
                "warmTier": STORAGE_STATE_ENABLED,
                "warmTierRetentionPeriod": { "numberOfDays": 90 },
                "disallowIngestNullNaN": false,
            }),
        },
        Case {
            // The cold tier is what the warm-tier period's documented
            // condition names, so the period is accepted with no `warmTier`
            // member of its own.
            label: "the cold tier, with a warm-tier period and no warm-tier state named",
            body: json!({
                "storageType": STORAGE_TYPE_MULTI_LAYER,
                "multiLayerStorage": multi_layer_storage("plant-4-archive", "ColdRole_4"),
                "warmTierRetentionPeriod": { "numberOfDays": 7 },
            }),
            expected: json!({
                "storageType": STORAGE_TYPE_MULTI_LAYER,
                "multiLayerStorage": multi_layer_storage("plant-4-archive", "ColdRole_4"),
                "disassociatedDataStorage": STORAGE_STATE_ENABLED,
                "retentionPeriod": { "unlimited": true },
                "warmTier": STORAGE_STATE_DISABLED,
                "warmTierRetentionPeriod": { "numberOfDays": 7 },
                "disallowIngestNullNaN": false,
            }),
        },
        Case {
            label: "the cold tier, with an unlimited warm tier and data streams enabled",
            body: json!({
                "storageType": STORAGE_TYPE_MULTI_LAYER,
                "multiLayerStorage": multi_layer_storage("plant-3-archive", "ColdRole-3"),
                "disassociatedDataStorage": STORAGE_STATE_ENABLED,
                "warmTier": STORAGE_STATE_ENABLED,
                "warmTierRetentionPeriod": { "unlimited": true },
                "disallowIngestNullNaN": true,
            }),
            expected: json!({
                "storageType": STORAGE_TYPE_MULTI_LAYER,
                "multiLayerStorage": multi_layer_storage("plant-3-archive", "ColdRole-3"),
                "disassociatedDataStorage": STORAGE_STATE_ENABLED,
                "retentionPeriod": { "unlimited": true },
                "warmTier": STORAGE_STATE_ENABLED,
                "warmTierRetentionPeriod": { "unlimited": true },
                "disallowIngestNullNaN": true,
            }),
        },
    ]
}

/// Every configuration the rule admits reads back through the describe: the put
/// answers with the setting it stored, the describe reports that same setting,
/// and the update date it reports is the instant of the put.
///
/// Each row runs against its own store, so the row order says nothing about the
/// answer; that a put REPLACES the setting whole is pinned separately below.
#[tokio::test]
async fn every_accepted_configuration_reads_back() {
    for case in round_trip_cases() {
        let label = case.label;
        let router = router();
        let before = now_seconds();

        let (status, body) = put(&router, case.body.clone()).await;
        assert_eq!(status, 200, "case {label:?}: put got {status} {body}");
        assert_eq!(
            reported(&body),
            case.expected,
            "case {label:?}: the put reported {body}"
        );
        let state = body["configurationStatus"]["state"].as_str().unwrap_or("");
        assert!(
            CONFIGURATION_STATE_VALUES.contains(&state),
            "case {label:?}: the put reported the unmodelled state {state:?}"
        );

        let (status, body) = describe(&router).await;
        assert_eq!(status, 200, "case {label:?}: describe got {status} {body}");
        assert_eq!(
            reported(&body),
            case.expected,
            "case {label:?}: the describe reported {body}"
        );
        let state = body["configurationStatus"]["state"].as_str().unwrap_or("");
        assert!(
            CONFIGURATION_STATE_VALUES.contains(&state),
            "case {label:?}: the describe reported the unmodelled state {state:?}"
        );
        let last_update = body["lastUpdateDate"].as_f64();
        let after = now_seconds();
        assert!(
            last_update.is_some_and(|at| at >= before - 1.0 && at <= after + 1.0),
            "case {label:?}: the describe reported lastUpdateDate {last_update:?}, expected the \
             instant of the put (between {before} and {after})"
        );
    }
}

/// A put is the last word on the account's storage: one naming the hot tier
/// alone drops the cold-tier destination, the warm tier and the retention
/// period an earlier put selected, rather than merging into them.
#[tokio::test]
async fn a_put_replaces_the_whole_setting() {
    let router = router();

    let (status, body) = put(
        &router,
        json!({
            "storageType": STORAGE_TYPE_MULTI_LAYER,
            "multiLayerStorage": multi_layer_storage("archive", "ColdRole"),
            "retentionPeriod": { "numberOfDays": 45 },
            "warmTier": STORAGE_STATE_ENABLED,
            "warmTierRetentionPeriod": { "numberOfDays": 120 },
            "disallowIngestNullNaN": true,
        }),
    )
    .await;
    assert_eq!(status, 200, "seed the cold tier: got {status} {body}");

    let (status, body) = put(&router, json!({ "storageType": STORAGE_TYPE_SITEWISE_DEFAULT })).await;
    assert_eq!(status, 200, "put the hot tier alone: got {status} {body}");
    assert_eq!(
        reported(&body),
        unconfigured(),
        "the put of the hot tier alone kept members of the configuration it replaced: got {body}"
    );

    let (_, body) = describe(&router).await;
    assert_eq!(
        reported(&body),
        unconfigured(),
        "the describe after the replacing put: got {body}"
    );
}

/// The configuration a put accepts is reported as still being applied, and the
/// clock — not another call — is what moves it into force: the same describe
/// reports it active once the span the service settles a transition in has
/// passed, with the configuration itself unchanged.
#[tokio::test]
async fn an_accepted_configuration_settles_into_force_on_the_clock() {
    let router = router();

    let body = json!({
        "storageType": STORAGE_TYPE_MULTI_LAYER,
        "multiLayerStorage": multi_layer_storage("settling-archive", "ColdRole"),
        "retentionPeriod": { "numberOfDays": 60 },
    });
    let started = Instant::now();
    let (status, answer) = put(&router, body).await;
    assert_eq!(status, 200, "put a cold-tier configuration: {status} {answer}");
    let expected = reported(&answer);
    assert_eq!(
        answer["configurationStatus"]["state"].as_str(),
        Some(CONFIGURATION_STATE_UPDATE_IN_PROGRESS),
        "the put of a fresh configuration reported {answer}, expected it to still be being applied"
    );

    let (_, answer) = describe(&router).await;
    let elapsed = started.elapsed();
    // Only an assertion about the span itself: a describe that got there before
    // the transition fell due must report the configuration as still being
    // applied. A loaded runner that took longer than the span to make two
    // in-process calls has nothing to say about the rule, so the timing is read
    // rather than assumed.
    if elapsed < ACTIVATION_DELAY {
        assert_eq!(
            answer["configurationStatus"]["state"].as_str(),
            Some(CONFIGURATION_STATE_UPDATE_IN_PROGRESS),
            "a describe taken {elapsed:?} after the put, inside the {ACTIVATION_DELAY:?} span, \
             reported {answer}, expected the configuration to still be being applied"
        );
    }

    // Comfortably past the span the service settles the owed transition in.
    tokio::time::sleep(ACTIVATION_DELAY + Duration::from_millis(200)).await;

    let (status, answer) = describe(&router).await;
    assert_eq!(status, 200, "describe after the settle: {status} {answer}");
    assert_eq!(
        answer["configurationStatus"]["state"].as_str(),
        Some(CONFIGURATION_STATE_ACTIVE),
        "after the settle the configuration reported {answer}"
    );
    assert_eq!(
        reported(&answer),
        expected,
        "the settle changed the configuration: got {answer}, expected {expected}"
    );
}

/// Disassociated data storage is the one member of this setting that does not
/// go back: once a put has enabled it, a put that would leave it anything else
/// is refused, and the setting stays as it was. A put that keeps naming it is
/// accepted.
#[tokio::test]
async fn enabled_disassociated_data_storage_cannot_be_disabled() {
    let router = router();

    let enabled = json!({
        "storageType": STORAGE_TYPE_SITEWISE_DEFAULT,
        "disassociatedDataStorage": STORAGE_STATE_ENABLED,
    });
    let (status, body) = put(&router, enabled.clone()).await;
    assert_eq!(status, 200, "enable disassociated data storage: {status} {body}");

    let rows = [
        (
            "a put naming it disabled",
            json!({
                "storageType": STORAGE_TYPE_SITEWISE_DEFAULT,
                "disassociatedDataStorage": STORAGE_STATE_DISABLED,
            }),
        ),
        (
            "a put that stops naming it",
            json!({ "storageType": STORAGE_TYPE_SITEWISE_DEFAULT }),
        ),
    ];
    for (label, body) in rows {
        let (status, answer) = put(&router, body.clone()).await;
        assert_eq!(
            status, 400,
            "case {label:?} ({body}): got {status} {answer}"
        );
        let code = error_code(&answer);
        assert!(
            matches!(code, "InvalidRequestException" | "ValidationException"),
            "case {label:?}: refused with {code:?}, expected a request refusal: {answer}"
        );

        let (_, answer) = describe(&router).await;
        assert_eq!(
            answer["disassociatedDataStorage"].as_str(),
            Some(STORAGE_STATE_ENABLED),
            "case {label:?}: the refusal changed the feature: got {answer}"
        );
    }

    let (status, body) = put(&router, enabled).await;
    assert_eq!(
        status, 200,
        "a put that keeps naming the feature enabled: got {status} {body}"
    );
    assert_eq!(
        body["disassociatedDataStorage"].as_str(),
        Some(STORAGE_STATE_ENABLED),
        "the accepted put dropped the feature: got {body}"
    );
}

/// A selection outside the rules the shapes state is refused — and the refusal
/// leaves the stored configuration exactly as it was.
#[tokio::test]
async fn a_selection_outside_the_rule_is_refused_and_stores_nothing() {
    let router = router();

    // The configuration every refusal below has to leave alone.
    let seeded = json!({
        "storageType": STORAGE_TYPE_MULTI_LAYER,
        "multiLayerStorage": multi_layer_storage("seeded-archive", "SeededRole"),
        "retentionPeriod": { "numberOfDays": 30 },
        "warmTier": STORAGE_STATE_ENABLED,
        "warmTierRetentionPeriod": { "numberOfDays": 60 },
    });
    let (status, body) = put(&router, seeded).await;
    assert_eq!(status, 200, "seed the configuration: {status} {body}");
    let in_force = reported(&body);

    let rows: Vec<(&str, Value)> = vec![
        (
            "a lowercase spelling of a modelled storage type",
            json!({ "storageType": "sitewise_default_storage" }),
        ),
        (
            "a storage tier the model does not define",
            json!({ "storageType": "COLD_STORAGE" }),
        ),
        ("the empty storage type", json!({ "storageType": "" })),
        (
            "the cold tier with no destination",
            json!({ "storageType": STORAGE_TYPE_MULTI_LAYER }),
        ),
        (
            "a destination beside the tier that keeps no cold copy",
            json!({
                "storageType": STORAGE_TYPE_SITEWISE_DEFAULT,
                "multiLayerStorage": multi_layer_storage("archive", "ColdRole"),
            }),
        ),
        (
            "a destination whose bucket is not an ARN",
            json!({
                "storageType": STORAGE_TYPE_MULTI_LAYER,
                "multiLayerStorage": {
                    "customerManagedS3Storage": {
                        "s3ResourceArn": "my-bucket",
                        "roleArn": "arn:aws:iam::000000000000:role/ColdRole",
                    }
                },
            }),
        ),
        (
            "a destination whose role ARN names a partition the modelled ARN excludes",
            json!({
                "storageType": STORAGE_TYPE_MULTI_LAYER,
                "multiLayerStorage": {
                    "customerManagedS3Storage": {
                        "s3ResourceArn": "arn:aws:s3:::archive",
                        "roleArn": "arn:aws-iso:iam::000000000000:role/ColdRole",
                    }
                },
            }),
        ),
        (
            "a retention period naming both a day count and unlimited",
            json!({
                "storageType": STORAGE_TYPE_SITEWISE_DEFAULT,
                "retentionPeriod": { "numberOfDays": 30, "unlimited": true },
            }),
        ),
        (
            "a retention period naming neither",
            json!({
                "storageType": STORAGE_TYPE_SITEWISE_DEFAULT,
                "retentionPeriod": {},
            }),
        ),
        (
            "a retention period that is neither unlimited nor a day count",
            json!({
                "storageType": STORAGE_TYPE_SITEWISE_DEFAULT,
                "retentionPeriod": { "unlimited": false },
            }),
        ),
        (
            "a retention period of zero days",
            json!({
                "storageType": STORAGE_TYPE_SITEWISE_DEFAULT,
                "retentionPeriod": { "numberOfDays": 0 },
            }),
        ),
        (
            "a retention period of a negative number of days",
            json!({
                "storageType": STORAGE_TYPE_SITEWISE_DEFAULT,
                "retentionPeriod": { "numberOfDays": -30 },
            }),
        ),
        (
            "a warm-tier period on the hot tier alone, with no cold tier",
            json!({
                "storageType": STORAGE_TYPE_SITEWISE_DEFAULT,
                "warmTierRetentionPeriod": { "numberOfDays": 90 },
            }),
        ),
        (
            "a warm-tier period on the hot tier beside an enabled warm tier",
            json!({
                "storageType": STORAGE_TYPE_SITEWISE_DEFAULT,
                "warmTier": STORAGE_STATE_ENABLED,
                "warmTierRetentionPeriod": { "numberOfDays": 90 },
            }),
        ),
        (
            "a warm-tier period naming both a day count and unlimited",
            json!({
                "storageType": STORAGE_TYPE_MULTI_LAYER,
                "multiLayerStorage": multi_layer_storage("archive", "ColdRole"),
                "warmTier": STORAGE_STATE_ENABLED,
                "warmTierRetentionPeriod": { "numberOfDays": 90, "unlimited": true },
            }),
        ),
        (
            "a warm-tier state the model does not define",
            json!({ "storageType": STORAGE_TYPE_SITEWISE_DEFAULT, "warmTier": "On" }),
        ),
        (
            "a data-stream state the model does not define",
            json!({
                "storageType": STORAGE_TYPE_SITEWISE_DEFAULT,
                "disassociatedDataStorage": "Enabled",
            }),
        ),
    ];

    for (label, body) in rows {
        let (status, answer) = put(&router, body.clone()).await;
        assert_eq!(
            status, 400,
            "case {label:?} ({body}): got {status} {answer}"
        );
        let code = error_code(&answer);
        assert!(
            matches!(code, "InvalidRequestException" | "ValidationException"),
            "case {label:?}: refused with {code:?}, expected a request refusal: {answer}"
        );

        let (_, answer) = describe(&router).await;
        assert_eq!(
            reported(&answer),
            in_force,
            "case {label:?}: the refusal changed the stored configuration: got {answer}"
        );
    }
}
