// SPDX-License-Identifier: CC0-1.0

//! What `DescribeStorageConfiguration` answers, and what it must leave alone.
//!
//! The rule these tests pin: the describe of the account-wide storage setting is
//! a REPORTING operation, so it answers from the stored setting — the required
//! members always filled, the state the account carries filled in for each
//! member the setting does not name, and the instant the last put recorded — and
//! it changes nothing a later caller sees: repeating it answers identically, an
//! account no put has configured still reports no update date however often it
//! is read, and the clock alone moves the status, so reads taken while a
//! configuration is still being applied neither settle it early nor hold it
//! back.
//!
//! The round trip through `PutStorageConfiguration` and the selections that
//! operation refuses are covered by `storage_configuration.rs`; these tests are
//! about the read alone, and reach that suite's helpers rather than copying
//! them.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::ACTIVATION_DELAY;

use crate::asset_lifecycle::router;
use crate::storage_configuration::{describe, multi_layer_storage, put, reported};

/// How many times each row is read: one answer is no evidence that reading
/// leaves the setting alone.
const READS: u32 = 4;

// The values below are written out as literals rather than taken from the
// crate's own constants, so a constant whose value drifted from the model would
// fail these assertions instead of agreeing with itself.

/// The `StorageType` that keeps data in the service-managed hot tier.
///
/// Source: `smithy_query(query="shape", shape="StorageType")`, which reports
/// `values: ["SITEWISE_DEFAULT_STORAGE", "MULTI_LAYER_STORAGE"]`.
const HOT_TIER: &str = "SITEWISE_DEFAULT_STORAGE";

/// The `StorageType` that also keeps a copy in a customer managed bucket; see
/// [`HOT_TIER`] for the source.
const COLD_TIER: &str = "MULTI_LAYER_STORAGE";

/// The `DisassociatedDataStorageState` / `WarmTierState` value that turns one of
/// those features on.
///
/// Source: `smithy_query(query="shape",
/// shape="DisassociatedDataStorageState,WarmTierState")`, which reports
/// `values: ["ENABLED", "DISABLED"]` for each.
const FEATURE_ON: &str = "ENABLED";

/// The value that turns one of them off; see [`FEATURE_ON`] for the source.
const FEATURE_OFF: &str = "DISABLED";

/// The `ConfigurationState` a configuration in force reports.
///
/// Source: `smithy_query(query="shape", shape="ConfigurationState")`, which
/// reports `values: ["ACTIVE", "UPDATE_IN_PROGRESS", "UPDATE_FAILED"]`.
const IN_FORCE: &str = "ACTIVE";

/// The `ConfigurationState` a configuration still being applied reports; see
/// [`IN_FORCE`] for the source.
const BEING_APPLIED: &str = "UPDATE_IN_PROGRESS";

/// The configuration an account no put has configured reports, written out from
/// the member documentation rather than from the crate: the hot tier
/// (`storageType` — "IoT SiteWise saves your data into the hot tier"), data kept
/// indefinitely there (`retentionPeriod` — "By default, your data is kept
/// indefinitely in the hot tier"), NULL/NaN ingestion allowed
/// (`disallowIngestNullNaN` — "By default the feature is allowed"), each tier
/// state as the account carries it — data streams no asset property owns
/// accepted (`disassociatedDataStorage` — "ENABLED – IoT SiteWise accepts time
/// series that aren't associated with asset properties", which is the ingest
/// such an account takes) and the warm tier, a tier an account opts into, off —
/// and nothing reported for the members that name a resource rather than a
/// state.
fn hot_tier_default() -> Value {
    json!({
        "storageType": HOT_TIER,
        "multiLayerStorage": Value::Null,
        "disassociatedDataStorage": FEATURE_ON,
        "retentionPeriod": { "unlimited": true },
        "warmTier": FEATURE_OFF,
        "warmTierRetentionPeriod": Value::Null,
        "disallowIngestNullNaN": false,
    })
}

/// Comfortably past [`ACTIVATION_DELAY`], the span this service settles an owed
/// transition in, so the status every read below sees is the settled one.
fn settled() -> Duration {
    ACTIVATION_DELAY + Duration::from_millis(200)
}

/// One row of the read table: the configuration the account is left in, and the
/// seven members every describe of it must report.
struct Row {
    label: &'static str,
    /// The put that configures the account, or `None` for an account no put has
    /// configured.
    put: Option<Value>,
    expected: Value,
}

/// The settings a describe has to report, spanning what the rule generalises
/// over: no configuration at all, each storage tier, both forms a retention
/// period takes, the warm tier with and without a period of its own, and the
/// members whose documented default the answer fills in.
fn rows() -> Vec<Row> {
    vec![
        Row {
            label: "an account no put has configured",
            put: None,
            expected: hot_tier_default(),
        },
        Row {
            label: "the hot tier, naming nothing else",
            put: Some(json!({ "storageType": HOT_TIER })),
            expected: hot_tier_default(),
        },
        Row {
            label: "the hot tier keeping 7 days, with NULL/NaN disallowed",
            put: Some(json!({
                "storageType": HOT_TIER,
                "retentionPeriod": { "numberOfDays": 7 },
                "disallowIngestNullNaN": true,
            })),
            expected: json!({
                "storageType": HOT_TIER,
                "multiLayerStorage": Value::Null,
                "disassociatedDataStorage": FEATURE_ON,
                "retentionPeriod": { "numberOfDays": 7 },
                "warmTier": FEATURE_OFF,
                "warmTierRetentionPeriod": Value::Null,
                "disallowIngestNullNaN": true,
            }),
        },
        Row {
            label: "the cold tier with a destination and an unlimited warm tier",
            put: Some(json!({
                "storageType": COLD_TIER,
                "multiLayerStorage": multi_layer_storage("read-side-archive", "ReadSideRole"),
                "disassociatedDataStorage": FEATURE_ON,
                "warmTier": FEATURE_ON,
                "warmTierRetentionPeriod": { "unlimited": true },
            })),
            expected: json!({
                "storageType": COLD_TIER,
                "multiLayerStorage": multi_layer_storage("read-side-archive", "ReadSideRole"),
                "disassociatedDataStorage": FEATURE_ON,
                "retentionPeriod": { "unlimited": true },
                "warmTier": FEATURE_ON,
                "warmTierRetentionPeriod": { "unlimited": true },
                "disallowIngestNullNaN": false,
            }),
        },
        Row {
            label: "the cold tier keeping 30 days hot and 90 days warm",
            put: Some(json!({
                "storageType": COLD_TIER,
                "multiLayerStorage": multi_layer_storage("read-side-archive/deep", "Cold_Role-9"),
                "retentionPeriod": { "numberOfDays": 30, "unlimited": false },
                "warmTier": FEATURE_ON,
                "warmTierRetentionPeriod": { "numberOfDays": 90 },
            })),
            expected: json!({
                "storageType": COLD_TIER,
                "multiLayerStorage": multi_layer_storage("read-side-archive/deep", "Cold_Role-9"),
                "disassociatedDataStorage": FEATURE_ON,
                "retentionPeriod": { "numberOfDays": 30, "unlimited": false },
                "warmTier": FEATURE_ON,
                "warmTierRetentionPeriod": { "numberOfDays": 90 },
                "disallowIngestNullNaN": false,
            }),
        },
    ]
}

/// Every setting a describe can be asked about reports the members the rule
/// says, and reading it repeatedly answers the same thing every time: the
/// configuration, the status and the update date are what the store holds, not
/// something a read changed.
#[tokio::test]
async fn every_setting_reads_back_the_same_on_every_describe() {
    for row in rows() {
        let label = row.label;
        let router = router();

        let configured = row.put.is_some();
        if let Some(body) = row.put {
            let (status, answer) = put(&router, body).await;
            assert_eq!(status, 200, "case {label:?}: the put got {status} {answer}");
        }
        if configured {
            // Let the transition the put owes fall due first, so a status that
            // moves between the reads below would be a read changing it rather
            // than the clock. An account no put has configured owes none, so
            // that row waits for nothing.
            tokio::time::sleep(settled()).await;
        }

        let mut first: Option<Value> = None;
        for attempt in 1..=READS {
            let (status, answer) = describe(&router).await;
            assert_eq!(
                status, 200,
                "case {label:?}: describe {attempt} got {status} {answer}"
            );
            assert_eq!(
                reported(&answer),
                row.expected,
                "case {label:?}: describe {attempt} reported {answer}, expected {}",
                row.expected
            );
            // `storageType` and `configurationStatus` are the two members the
            // model marks required, so every answer carries them. Past the
            // settling span the state is the settled one, which is also the
            // value of the modelled enum this read must report.
            let state = answer["configurationStatus"]["state"].as_str().unwrap_or("");
            assert_eq!(
                state, IN_FORCE,
                "case {label:?}: describe {attempt} taken past the {ACTIVATION_DELAY:?} settling \
                 span reported {answer}, expected the configuration in force"
            );
            assert_eq!(
                answer["lastUpdateDate"].is_null(),
                !configured,
                "case {label:?}: describe {attempt} reported {answer}, expected an update date \
                 only once a put had recorded one (configured: {configured})"
            );

            match &first {
                None => first = Some(answer),
                Some(expected) => assert_eq!(
                    &answer, expected,
                    "case {label:?}: describe {attempt} answered {answer}, expected the answer \
                     the first read saw, {expected} — a read must not change what a later caller \
                     sees"
                ),
            }
        }
    }
}

/// The status a describe reports moves on the CLOCK and on nothing else: reads
/// taken while the configuration is still being applied report it as such and do
/// not settle it early, and the read that comes after the span reports it in
/// force with the configuration and the update date the put recorded.
#[tokio::test]
async fn reads_inside_the_settling_span_do_not_settle_a_configuration_early() {
    let router = router();

    let started = Instant::now();
    let (status, answer) = put(
        &router,
        json!({
            "storageType": COLD_TIER,
            "multiLayerStorage": multi_layer_storage("settling-read-archive", "SettlingRole"),
            "retentionPeriod": { "numberOfDays": 15 },
        }),
    )
    .await;
    assert_eq!(status, 200, "the put got {status} {answer}");
    let expected = reported(&answer);
    // The span is never zero, so the put's own answer is inside it whatever the
    // runner's speed: this is the assertion about the transient state that
    // cannot be skipped.
    assert_eq!(
        answer["configurationStatus"]["state"].as_str(),
        Some(BEING_APPLIED),
        "the put reported {answer}, expected the configuration it accepted to still be being \
         applied"
    );

    let mut recorded_update: Option<Value> = None;
    for attempt in 1..=READS {
        let (status, answer) = describe(&router).await;
        let elapsed = started.elapsed();
        assert_eq!(
            status, 200,
            "describe {attempt} inside the span got {status} {answer}"
        );
        // Only an assertion about the span itself: a loaded runner that took
        // longer than it to make these in-process calls has nothing to say about
        // the rule, so the timing is read rather than assumed. The put's answer
        // above is what keeps the transient state asserted on such a runner.
        if elapsed < ACTIVATION_DELAY {
            assert_eq!(
                answer["configurationStatus"]["state"].as_str(),
                Some(BEING_APPLIED),
                "describe {attempt}, taken {elapsed:?} after the put and so inside the \
                 {ACTIVATION_DELAY:?} span, reported {answer}, expected the configuration to \
                 still be being applied"
            );
        }
        assert_eq!(
            reported(&answer),
            expected,
            "describe {attempt} reported {answer}, expected the configuration the put stored, \
             {expected}"
        );
        let update = answer["lastUpdateDate"].clone();
        assert!(
            !update.is_null(),
            "describe {attempt} reported no update date for a configuration a put recorded: \
             {answer}"
        );
        match &recorded_update {
            None => recorded_update = Some(update),
            Some(first) => assert_eq!(
                &update, first,
                "describe {attempt} reported the update date {update}, expected the instant of \
                 the put, {first} — a read does not update the configuration"
            ),
        }
    }

    tokio::time::sleep(settled()).await;

    let (status, answer) = describe(&router).await;
    assert_eq!(status, 200, "the describe past the span got {status} {answer}");
    assert_eq!(
        answer["configurationStatus"]["state"].as_str(),
        Some(IN_FORCE),
        "a describe past the {ACTIVATION_DELAY:?} span reported {answer}, expected the \
         configuration in force"
    );
    assert_eq!(
        reported(&answer),
        expected,
        "the settle changed the configuration: got {answer}, expected {expected}"
    );
    assert_eq!(
        Some(answer["lastUpdateDate"].clone()),
        recorded_update,
        "the settle changed the update date: got {answer}, expected {recorded_update:?}"
    );
}
