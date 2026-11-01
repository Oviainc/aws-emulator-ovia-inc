// SPDX-License-Identifier: CC0-1.0

//! What the account-wide configuration surface REPORTS: the `ConfigurationState`
//! each setting is in, and the tier state a storage answer carries.
//!
//! Two rules, driven over HTTP through the same router the emulator boots with:
//!
//! 1. Setting the account's default encryption takes effect with the call, so
//!    the put's own answer and every read after it report the configuration in
//!    force (`ACTIVE`) — whatever encryption type and key identifier the put
//!    named, and however much later the read is taken. No call and no wait moves
//!    that state.
//! 2. Every member of the storage configuration that carries a tier STATE is
//!    reported on every answer: the state the last put named it, or the state the
//!    account is in while no put has named it. The second half is pinned against
//!    BEHAVIOUR rather than against a literal alone — the state a read reports is
//!    the one ingestion and the bulk-import tier precondition act on.
//!
//! The request helpers are the ones `asset_lifecycle` and `storage_configuration`
//! define, re-used rather than copied.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::ACTIVATION_DELAY;

use crate::asset_lifecycle::{call, error_code, router};
use crate::storage_configuration::{describe as describe_storage, put as put_storage};
use crate::support::now_seconds;

// The wire values below are written out as literals rather than read from the
// crate under test, so a constant that drifted from the model would fail these
// assertions instead of agreeing with itself.

/// `POST`/`GET /configuration/account/encryption`, from
/// `smithy_query(query="operation", operation="PutDefaultEncryptionConfiguration")`.
const ENCRYPTION_URI: &str = "/configuration/account/encryption";

/// `POST /jobs`, the route `CreateBulkImportJob` is bound to.
const JOBS_URI: &str = "/jobs";

/// The `EncryptionType` values. Source: `smithy_query(query="shape",
/// shape="EncryptionType")` — `["SITEWISE_DEFAULT_ENCRYPTION",
/// "KMS_BASED_ENCRYPTION"]`.
const SERVICE_KEY: &str = "SITEWISE_DEFAULT_ENCRYPTION";
const CUSTOMER_KEY: &str = "KMS_BASED_ENCRYPTION";

/// The `ConfigurationState` of a configuration in force. Source:
/// `smithy_query(query="shape", shape="ConfigurationState")` —
/// `["ACTIVE", "UPDATE_IN_PROGRESS", "UPDATE_FAILED"]`.
const IN_FORCE: &str = "ACTIVE";

/// The `StorageType` that keeps data in the service-managed hot tier. Source:
/// `smithy_query(query="shape", shape="StorageType")` —
/// `["SITEWISE_DEFAULT_STORAGE", "MULTI_LAYER_STORAGE"]`.
const HOT_TIER: &str = "SITEWISE_DEFAULT_STORAGE";

/// The two `DisassociatedDataStorageState` / `WarmTierState` values. Source:
/// `smithy_query(query="shape",
/// shape="DisassociatedDataStorageState,WarmTierState")` — `["ENABLED",
/// "DISABLED"]` for each.
const ON: &str = "ENABLED";
const OFF: &str = "DISABLED";

/// The state an account that has never named `disassociatedDataStorage` is in:
/// IoT SiteWise accepts time series that aren't associated with asset
/// properties, which is the behaviour
/// `the_tier_state_a_read_reports_is_the_one_the_service_acts_on` below drives.
const UNNAMED_DISASSOCIATED_DATA_STORAGE: &str = ON;

/// The state an account that has never named `warmTier` is in: the warm tier is
/// a tier an account opts into, so nothing has provisioned it and the
/// bulk-import precondition that needs it is not met — again driven below.
const UNNAMED_WARM_TIER: &str = OFF;

/// How many times a setting is read: one answer is no evidence that reading
/// leaves it alone.
const READS: u32 = 3;

/// A single-Region KMS key id, in the form KMS mints one, from the `kmsKeyId`
/// documentation's own key-identifier forms.
const KEY_ID: &str = "1234abcd-12ab-34cd-56ef-1234567890ab";

/// Put the account's default encryption configuration.
async fn put_encryption(router: &MultiServiceRouter, body: Value) -> (u16, Value) {
    call(router, "POST", ENCRYPTION_URI, Some(body)).await
}

/// Read the account's default encryption configuration.
async fn describe_encryption(router: &MultiServiceRouter) -> (u16, Value) {
    call(router, "GET", ENCRYPTION_URI, None).await
}

/// The `configurationStatus.state` an answer reports.
fn state(body: &Value) -> &str {
    body["configurationStatus"]["state"].as_str().unwrap_or("")
}

/// The encryption configurations a put can name, spanning both encryption types
/// and every key-identifier form a customer managed key is named by: the state
/// the rule is about must not depend on which of them a put chose.
fn encryption_puts() -> Vec<(&'static str, Value)> {
    vec![
        (
            "the service-owned key, naming no key identifier",
            json!({ "encryptionType": SERVICE_KEY }),
        ),
        (
            "a customer managed key named by key id",
            json!({ "encryptionType": CUSTOMER_KEY, "kmsKeyId": KEY_ID }),
        ),
        (
            "a customer managed key named by multi-Region key id",
            json!({
                "encryptionType": CUSTOMER_KEY,
                "kmsKeyId": format!("mrk-{}", "a1b2c3d4".repeat(4)),
            }),
        ),
        (
            "a customer managed key named by alias path",
            json!({ "encryptionType": CUSTOMER_KEY, "kmsKeyId": "alias/Plant_1" }),
        ),
        (
            "a customer managed key named by its full ARN",
            json!({
                "encryptionType": CUSTOMER_KEY,
                "kmsKeyId": format!("arn:aws:kms:eu-west-1:210987654321:key/{KEY_ID}"),
            }),
        ),
    ]
}

/// Every configuration a put accepts is in force on the put's own answer, and
/// stays so on every read after it: a client that waits for the state to settle
/// has nothing to wait for.
#[tokio::test]
async fn an_accepted_encryption_configuration_is_in_force_on_its_own_answer() {
    for (case, body) in encryption_puts() {
        // One account per row: the setting is account-wide, so a row must not
        // read what the row before it left behind.
        let router = router();

        let (status, answer) = put_encryption(&router, body.clone()).await;
        assert_eq!(status, 200, "case {case:?}: the put got {status} {answer}");
        assert_eq!(
            state(&answer),
            IN_FORCE,
            "case {case:?}: the put reported state {:?}, expected the configuration it applied \
             to be in force: {answer}",
            state(&answer)
        );

        let mut first: Option<Value> = None;
        for attempt in 1..=READS {
            let (status, answer) = describe_encryption(&router).await;
            assert_eq!(
                status, 200,
                "case {case:?}: describe {attempt} got {status} {answer}"
            );
            assert_eq!(
                state(&answer),
                IN_FORCE,
                "case {case:?}: describe {attempt} reported state {:?}, expected the \
                 configuration to be in force: {answer}",
                state(&answer)
            );
            match &first {
                None => first = Some(answer),
                Some(expected) => assert_eq!(
                    &answer, expected,
                    "case {case:?}: describe {attempt} answered {answer}, expected the answer \
                     the first read saw, {expected} — a read must not change what a later \
                     caller sees"
                ),
            }
        }
    }
}

/// The clock does not move this setting either: the answer a read gives before
/// the span this service settles an owed transition in is the answer it gives
/// after it.
#[tokio::test]
async fn the_encryption_configuration_a_put_stored_does_not_move_on_the_clock() {
    let router = router();

    let (status, answer) = put_encryption(
        &router,
        json!({ "encryptionType": CUSTOMER_KEY, "kmsKeyId": KEY_ID }),
    )
    .await;
    assert_eq!(status, 200, "the put got {status} {answer}");

    let (_, before) = describe_encryption(&router).await;
    assert_eq!(
        state(&before),
        IN_FORCE,
        "the read taken straight after the put reported {before}, expected the configuration \
         in force"
    );

    // Comfortably past the span this service settles a transition the clock owes
    // in: this setting owes none, so nothing here may change.
    tokio::time::sleep(ACTIVATION_DELAY + Duration::from_millis(200)).await;

    let (status, after) = describe_encryption(&router).await;
    assert_eq!(status, 200, "the read past the span got {status} {after}");
    assert_eq!(
        after, before,
        "the read taken past the {ACTIVATION_DELAY:?} span answered {after}, expected the \
         answer the earlier read saw, {before}"
    );
}

/// One row of the tier-state table: what a put names for the two members, and
/// the state every answer must then report for each.
struct Tiers {
    case: &'static str,
    /// What the put names, or `None` for an account no put has configured.
    named: Option<(Option<&'static str>, Option<&'static str>)>,
    expected_disassociated: &'static str,
    expected_warm_tier: &'static str,
}

/// The rows span what the rule generalises over: the account no put has
/// configured, a put naming neither member, a put naming each one alone at each
/// of its values, and a put naming both.
fn tier_rows() -> Vec<Tiers> {
    let mut rows = vec![
        Tiers {
            case: "an account no put has configured",
            named: None,
            expected_disassociated: UNNAMED_DISASSOCIATED_DATA_STORAGE,
            expected_warm_tier: UNNAMED_WARM_TIER,
        },
        Tiers {
            case: "a put naming neither member",
            named: Some((None, None)),
            expected_disassociated: UNNAMED_DISASSOCIATED_DATA_STORAGE,
            expected_warm_tier: UNNAMED_WARM_TIER,
        },
    ];
    for value in [ON, OFF] {
        rows.push(Tiers {
            case: if value == ON {
                "a put naming data streams on and no warm tier"
            } else {
                "a put naming data streams off and no warm tier"
            },
            named: Some((Some(value), None)),
            expected_disassociated: value,
            expected_warm_tier: UNNAMED_WARM_TIER,
        });
        rows.push(Tiers {
            case: if value == ON {
                "a put naming the warm tier on and no data-stream state"
            } else {
                "a put naming the warm tier off and no data-stream state"
            },
            named: Some((None, Some(value))),
            expected_disassociated: UNNAMED_DISASSOCIATED_DATA_STORAGE,
            expected_warm_tier: value,
        });
    }
    rows.push(Tiers {
        case: "a put naming data streams on and the warm tier off",
        named: Some((Some(ON), Some(OFF))),
        expected_disassociated: ON,
        expected_warm_tier: OFF,
    });
    rows.push(Tiers {
        case: "a put naming data streams off and the warm tier on",
        named: Some((Some(OFF), Some(ON))),
        expected_disassociated: OFF,
        expected_warm_tier: ON,
    });
    rows
}

/// Both tier-state members are reported on every answer of the storage
/// configuration — the put's own and every read after it — carrying the state
/// the put named or, where it named none, the state the account is in.
#[tokio::test]
async fn every_storage_answer_reports_both_tier_states() {
    for row in tier_rows() {
        let case = row.case;
        // One account per row, as above.
        let router = router();

        if let Some((disassociated, warm_tier)) = row.named {
            let mut body = json!({ "storageType": HOT_TIER });
            if let Some(value) = disassociated {
                body["disassociatedDataStorage"] = json!(value);
            }
            if let Some(value) = warm_tier {
                body["warmTier"] = json!(value);
            }
            let (status, answer) = put_storage(&router, body.clone()).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the put {body} got {status} {answer}"
            );
            assert_eq!(
                answer["disassociatedDataStorage"].as_str(),
                Some(row.expected_disassociated),
                "case {case:?}: the put reported disassociatedDataStorage {:?}, expected {:?}: \
                 {answer}",
                answer["disassociatedDataStorage"],
                row.expected_disassociated
            );
            assert_eq!(
                answer["warmTier"].as_str(),
                Some(row.expected_warm_tier),
                "case {case:?}: the put reported warmTier {:?}, expected {:?}: {answer}",
                answer["warmTier"],
                row.expected_warm_tier
            );
        }

        for attempt in 1..=READS {
            let (status, answer) = describe_storage(&router).await;
            assert_eq!(
                status, 200,
                "case {case:?}: describe {attempt} got {status} {answer}"
            );
            assert_eq!(
                answer["disassociatedDataStorage"].as_str(),
                Some(row.expected_disassociated),
                "case {case:?}: describe {attempt} reported disassociatedDataStorage {:?}, \
                 expected {:?}: {answer}",
                answer["disassociatedDataStorage"],
                row.expected_disassociated
            );
            assert_eq!(
                answer["warmTier"].as_str(),
                Some(row.expected_warm_tier),
                "case {case:?}: describe {attempt} reported warmTier {:?}, expected {:?}: \
                 {answer}",
                answer["warmTier"],
                row.expected_warm_tier
            );
        }
    }
}

/// Ingest one reading addressed by `alias` — a stream no asset property is
/// associated with — and answer whether the entry was accepted.
async fn alias_ingest_accepted(router: &MultiServiceRouter, alias: &str) -> bool {
    let (status, body) = call(
        router,
        "POST",
        "/properties",
        Some(json!({
            "entries": [{
                "entryId": "probe",
                "propertyAlias": alias,
                "propertyValues": [{
                    "value": { "doubleValue": 42.0 },
                    // Inside `[now - 7 days, now + 10 minutes]`, the window
                    // BatchPutAssetPropertyValue ingests a TQV in.
                    "timestamp": { "timeInSeconds": now_seconds() - 60 },
                }],
            }],
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "BatchPutAssetPropertyValue for {alias}: got {status} {body}"
    );
    body["errorEntries"]
        .as_array()
        .unwrap_or_else(|| panic!("no errorEntries in {body}"))
        .is_empty()
}

/// Ask for a bulk import job ingesting into time series OUTSIDE a workspace —
/// the form the account's warm or cold tier is a precondition for — and answer
/// `(status, body)`.
async fn import_outside_a_workspace(router: &MultiServiceRouter) -> (u16, Value) {
    call(
        router,
        "POST",
        JOBS_URI,
        Some(json!({
            "jobName": "Tier probe",
            "jobRoleArn": "arn:aws:iam::000000000000:role/SiteWiseImport",
            "files": [{ "bucket": "sitewise-import", "key": "turbines/2026.csv" }],
            "errorReportLocation": { "bucket": "sitewise-errors", "prefix": "imports/" },
        })),
    )
    .await
}

/// The tier state a read reports is the one the service ACTS on, for an account
/// no put has configured as much as for one a put has: data streams no asset
/// property owns are taken exactly while the read says they are, and a bulk
/// import outside a workspace is served exactly while the read says the warm
/// tier is on.
#[tokio::test]
async fn the_tier_state_a_read_reports_is_the_one_the_service_acts_on() {
    // (case, what a put names for the pair, whether alias-addressed ingest is
    // then taken, whether an import outside a workspace is then served).
    let rows: Vec<(&str, Option<Value>, bool, bool)> = vec![
        ("an account no put has configured", None, true, false),
        (
            "a put naming data streams off and the warm tier on",
            Some(json!({
                "storageType": HOT_TIER,
                "disassociatedDataStorage": OFF,
                "warmTier": ON,
            })),
            false,
            true,
        ),
        (
            "a put naming data streams on and the warm tier off",
            Some(json!({
                "storageType": HOT_TIER,
                "disassociatedDataStorage": ON,
                "warmTier": OFF,
            })),
            true,
            false,
        ),
    ];

    for (case, configuration, takes_ingest, serves_import) in rows {
        let router = router();
        if let Some(body) = configuration {
            let (status, answer) = put_storage(&router, body.clone()).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the put {body} got {status} {answer}"
            );
        }

        let (status, reported) = describe_storage(&router).await;
        assert_eq!(status, 200, "case {case:?}: describe got {status} {reported}");
        let disassociated = reported["disassociatedDataStorage"].as_str().unwrap_or("");
        let warm_tier = reported["warmTier"].as_str().unwrap_or("");
        assert_eq!(
            disassociated == ON,
            takes_ingest,
            "case {case:?}: the read reports disassociatedDataStorage {disassociated:?}, which \
             disagrees with the ingest the service takes: {reported}"
        );
        assert_eq!(
            warm_tier == ON,
            serves_import,
            "case {case:?}: the read reports warmTier {warm_tier:?}, which disagrees with the \
             import the service serves: {reported}"
        );

        let accepted = alias_ingest_accepted(&router, "/plant/line7/mixer/pressure").await;
        assert_eq!(
            accepted, takes_ingest,
            "case {case:?}: alias-addressed ingest was {}, expected the state the read reports \
             ({disassociated:?}) to decide it",
            if accepted { "accepted" } else { "refused" }
        );

        let (status, answer) = import_outside_a_workspace(&router).await;
        if serves_import {
            assert_eq!(
                status, 202,
                "case {case:?}: an import outside a workspace got {status} {answer}, expected \
                 the warm tier the read reports ({warm_tier:?}) to admit it"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: an import outside a workspace got {status} {answer}, expected \
                 the warm tier the read reports ({warm_tier:?}) to refuse it"
            );
            let code = error_code(&answer);
            assert_eq!(
                code, "InvalidRequestException",
                "case {case:?}: the import was refused with {code:?}: {answer}"
            );
        }
    }
}
