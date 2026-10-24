// SPDX-License-Identifier: CC0-1.0

//! `PutInstanceCommunicationLimits` and `GetInstanceCommunicationLimits`, end to end
//! over the wire.
//!
//! Every case drives real HTTP through the router the production entrypoint serves,
//! so each assertion exercises the restJson1 wire protocol, the modelled constraints
//! and the handlers together. The rules under test:
//!
//! * the put REPLACES the instance's `communicationLimitsConfig` with the config it
//!   was given, whatever that instance held before, and the stored config is what
//!   every later read of the instance projects — verbatim, for any config the model
//!   admits and any `InstanceId`;
//! * the put is idempotent ("This API is idempotent", per the model's own
//!   documentation): the same config again leaves the same config stored;
//! * the get only REPORTS: repeated reads agree, reading configures nothing, and an
//!   onboarded instance nobody has configured carries no `communicationLimitsConfig`
//!   at all (the modelled member is optional);
//! * instance-wide limits are configuration OF the Connect instance, so an instance
//!   with no finished onboarding job — never onboarded, or a job still `IN_PROGRESS`
//!   — is HTTP 404 `ResourceNotFoundException` on both operations, and the refused
//!   put stores nothing;
//! * a config outside `CommunicationLimit`'s modelled constraints (`frequency` in
//!   1..30, `maxCountPerRecipient` at least 1, `unit` from
//!   `CommunicationLimitTimeUnit`, a `communicationLimitsList` of at most 2) is HTTP
//!   400 `ValidationException` and leaves the stored config alone;
//! * each instance's config is its own: configuring one instance leaves every other
//!   instance's config untouched.

use serde_json::{json, Value};
use service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::ops::types::{
    CommunicationLimits, InstanceCommunicationLimitsConfig, COMMUNICATION_LIMIT_TIME_UNIT_VALUES,
};
use service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::state::{
    ComAmazonawsConnectcampaignsv2Amazonconnectcampaignservicev2State, ONBOARDING_JOB_DURATION,
};
use smithy_server_runtime::MultiServiceRouter;

use super::common::{call, error_code, router, router_with_engine};

/// The region every case signs for. Nothing in an instance's communication-limits
/// config is region-dependent, so the cases that do not vary it use this one.
const REGION: &str = "us-east-1";

/// One `CommunicationLimit` entry.
fn limit(max_count_per_recipient: i64, frequency: i64, unit: &str) -> Value {
    json!({
        "maxCountPerRecipient": max_count_per_recipient,
        "frequency": frequency,
        "unit": unit,
    })
}

/// The one `CommunicationLimitTimeUnit` the model's catalog declares.
fn day() -> &'static str {
    COMMUNICATION_LIMIT_TIME_UNIT_VALUES[0]
}

/// The limit entries a stored config holds, as `(maxCountPerRecipient, frequency,
/// unit)` triples — `None` when the instance holds no config at all, and an empty
/// list when it holds one that names no per-channel limits.
fn stored_entries(
    config: Option<&InstanceCommunicationLimitsConfig>,
) -> Option<Vec<(i32, i32, String)>> {
    config.map(|config| match config.all_channel_subtypes.as_ref() {
        Some(CommunicationLimits::communicationLimitsList(list)) => list
            .iter()
            .map(|entry| {
                (
                    entry.max_count_per_recipient,
                    entry.frequency,
                    entry.unit.clone(),
                )
            })
            .collect(),
        None => Vec::new(),
    })
}

/// Start `instance`'s onboarding job and wait out the span the clock owes it, so the
/// instance has a config to configure by the time this returns. Nothing in the
/// emulator sleeps for that span — only a caller that wants the settled state does.
async fn onboard(router: &MultiServiceRouter, region: &str, instance: &str) {
    let (status, body) = start_onboarding(router, region, instance).await;
    assert_eq!(
        status, 200,
        "case {instance:?}: expected onboarding to start, got {status} {body}"
    );
    tokio::time::sleep(ONBOARDING_JOB_DURATION + std::time::Duration::from_millis(150)).await;
}

/// `StartInstanceOnboardingJob` for `instance`, without waiting for the job.
async fn start_onboarding(
    router: &MultiServiceRouter,
    region: &str,
    instance: &str,
) -> (u16, Value) {
    call(
        router,
        region,
        "PUT",
        &format!("/v2/connect-instance/{instance}/onboarding"),
        &json!({"encryptionConfig": {"enabled": false}}),
    )
    .await
}

/// `PutInstanceCommunicationLimits` for `instance` with `config`.
async fn put_limits(
    router: &MultiServiceRouter,
    region: &str,
    instance: &str,
    config: &Value,
) -> (u16, Value) {
    call(
        router,
        region,
        "PUT",
        &format!("/v2/connect-instance/{instance}/communication-limits"),
        &json!({"communicationLimitsConfig": config}),
    )
    .await
}

/// `GetInstanceCommunicationLimits` for `instance`.
async fn get_limits(router: &MultiServiceRouter, region: &str, instance: &str) -> (u16, Value) {
    call(
        router,
        region,
        "GET",
        &format!("/v2/connect-instance/{instance}/communication-limits"),
        &Value::Null,
    )
    .await
}

/// The `communicationLimitsConfig` a successful read projects, asserting the read
/// succeeded.
async fn read_config(
    router: &MultiServiceRouter,
    region: &str,
    instance: &str,
    case: &str,
) -> Value {
    let (status, body) = get_limits(router, region, instance).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetInstanceCommunicationLimits expected 200, got {status} {body}"
    );
    body.get("communicationLimitsConfig")
        .cloned()
        .unwrap_or(Value::Null)
}

/// Instance ids spanning `InstanceId` (len 1..256, pattern `^[-_.a-zA-Z0-9]+$`, from
/// `smithy_query(query="operation", operation="GetInstanceCommunicationLimits")`):
/// the shortest id, the uuid form a Connect instance id takes, every punctuation
/// character the pattern admits, and the longest id the length bound allows.
fn instance_ids() -> Vec<(&'static str, String)> {
    vec![
        ("one character (length minimum)", "a".to_string()),
        (
            "uuid form",
            "11111111-2222-3333-4444-555555555555".to_string(),
        ),
        ("every admitted punctuation", "A-z_0.9".to_string()),
        ("256 characters (length maximum)", "i".repeat(256)),
    ]
}

/// The stored config is whatever the last accepted put carried, over a TABLE of
/// configs, and every read projects it verbatim.
///
/// The rows differ along what the rule generalises over: how many entries the
/// `communicationLimitsList` carries (its `@length` max is 2, and it has no minimum),
/// how many successive puts the instance takes, the `unit` (source:
/// `COMMUNICATION_LIMIT_TIME_UNIT_VALUES`, the model's `CommunicationLimitTimeUnit`
/// catalog), both published bounds of `frequency`'s `@range` 1..30 together with
/// `maxCountPerRecipient`'s minimum of 1, and a config carrying no member at all.
#[tokio::test]
async fn a_put_config_is_stored_and_every_read_projects_it() {
    let router = router();
    let unit = day();

    // (case, the configs to put in order)
    let cases: Vec<(&str, Vec<Value>)> = vec![
        (
            "one limit",
            vec![json!({
                "allChannelSubtypes": {"communicationLimitsList": [limit(5, 7, unit)]}
            })],
        ),
        (
            "two entries, the @length maximum of communicationLimitsList",
            vec![json!({
                "allChannelSubtypes": {
                    "communicationLimitsList": [limit(2, 1, unit), limit(10, 30, unit)]
                }
            })],
        ),
        (
            "an empty list: communicationLimitsList has no @length minimum",
            vec![json!({"allChannelSubtypes": {"communicationLimitsList": []}})],
        ),
        (
            "an empty config: the shape has no required member",
            vec![json!({})],
        ),
        (
            "both published bounds of frequency's @range 1..30 and the minimum count",
            vec![
                json!({
                    "allChannelSubtypes": {"communicationLimitsList": [limit(1, 1, unit)]}
                }),
                json!({
                    "allChannelSubtypes": {"communicationLimitsList": [limit(1, 30, unit)]}
                }),
            ],
        ),
        (
            "a later put replaces the whole config with a smaller one",
            vec![
                json!({
                    "allChannelSubtypes": {
                        "communicationLimitsList": [limit(3, 2, unit), limit(4, 3, unit)]
                    }
                }),
                json!({"allChannelSubtypes": {"communicationLimitsList": [limit(9, 14, unit)]}}),
            ],
        ),
        (
            "a large maxCountPerRecipient",
            vec![json!({
                "allChannelSubtypes": {"communicationLimitsList": [limit(1_000_000, 30, unit)]}
            })],
        ),
    ];

    for (index, (case, sequence)) in cases.iter().enumerate() {
        let instance = format!("limits-instance-{index}");
        onboard(&router, REGION, &instance).await;

        for config in sequence {
            let (status, body) = put_limits(&router, REGION, &instance, config).await;
            assert_eq!(
                status, 200,
                "case {case:?}: PutInstanceCommunicationLimits expected 200, got {status} {body}"
            );

            let stored = read_config(&router, REGION, &instance, case).await;
            assert_eq!(
                &stored, config,
                "case {case:?}: expected the read to project the config just put \
                 ({config}), got {stored}"
            );
        }

        let last = sequence.last().expect("every case puts at least one config");

        // Idempotent: the same config again is the same write, and still a 200.
        let (status, body) = put_limits(&router, REGION, &instance, last).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a repeated put expected 200, got {status} {body}"
        );
        let stored = read_config(&router, REGION, &instance, case).await;
        assert_eq!(
            &stored, last,
            "case {case:?}: expected the repeat to leave {last} stored, got {stored}"
        );

        // Reporting is not configuring: a second read of the same record agrees with
        // the first, so polling never changes what a later caller sees.
        let again = read_config(&router, REGION, &instance, case).await;
        assert_eq!(
            again, stored,
            "case {case:?}: expected two reads to agree, got {again} then {stored}"
        );
    }
}

/// The round trip holds for every `InstanceId` the model admits, each instance's
/// config is its own, and the store — not the response — is where the config lives.
#[tokio::test]
async fn each_instance_holds_its_own_config_whatever_its_id() {
    let (router, engine) = router_with_engine();
    let unit = day();

    // Every instance is onboarded first, so a later put cannot be read as having
    // onboarded one.
    for (_case, instance) in instance_ids() {
        onboard(&router, "eu-west-1", &instance).await;
    }

    for (index, (case, instance)) in instance_ids().into_iter().enumerate() {
        let count = i64::try_from(index).expect("a small index") + 1;
        let config = json!({
            "allChannelSubtypes": {"communicationLimitsList": [limit(count, count, unit)]}
        });
        let (status, body) = put_limits(&router, "eu-west-1", &instance, &config).await;
        assert_eq!(
            status, 200,
            "case {case:?}: PutInstanceCommunicationLimits expected 200, got {status} {body}"
        );

        let stored = read_config(&router, "eu-west-1", &instance, case).await;
        assert_eq!(
            stored, config,
            "case {case:?}: expected the read to project {config}, got {stored}"
        );

        // The record is the source of truth: the config the call carried, entry for
        // entry, is what the store holds for this instance.
        let state =
            engine.state::<ComAmazonawsConnectcampaignsv2Amazonconnectcampaignservicev2State>();
        let held = state
            .lock()
            .instance_onboarding(&instance)
            .map(|record| stored_entries(record.communication_limits.as_ref()));
        let expected = Some(Some(vec![(
            i32::try_from(count).expect("a small count"),
            i32::try_from(count).expect("a small count"),
            unit.to_string(),
        )]));
        assert_eq!(
            held, expected,
            "case {case:?}: expected the put to persist {expected:?} on the instance record, \
             got {held:?}"
        );
    }

    // An instance configured above does not configure any other: the ids that come
    // later in the table were untouched when the earlier ones were written, so each
    // one still reads back the config meant for it.
    for (index, (case, instance)) in instance_ids().into_iter().enumerate() {
        let count = i64::try_from(index).expect("a small index") + 1;
        let expected = json!({
            "allChannelSubtypes": {"communicationLimitsList": [limit(count, count, unit)]}
        });
        let stored = read_config(&router, "eu-west-1", &instance, case).await;
        assert_eq!(
            stored, expected,
            "case {case:?}: expected each instance to keep its own config {expected}, got {stored}"
        );
    }
}

/// An onboarded instance nobody has configured carries no config at all, and reading
/// it does not create one.
#[tokio::test]
async fn an_unconfigured_instance_reports_no_config() {
    let (router, engine) = router_with_engine();
    let instance = "unconfigured-instance";
    onboard(&router, REGION, instance).await;

    for read in 1..=2 {
        let (status, body) = get_limits(&router, REGION, instance).await;
        assert_eq!(
            status, 200,
            "case \"unconfigured instance\" read {read}: expected HTTP 200, got {status} {body}"
        );
        assert!(
            body.get("communicationLimitsConfig").is_none_or(Value::is_null),
            "case \"unconfigured instance\" read {read}: expected no communicationLimitsConfig, \
             got {body}"
        );
    }

    // And the read wrote nothing: the record still holds no config.
    let state = engine.state::<ComAmazonawsConnectcampaignsv2Amazonconnectcampaignservicev2State>();
    let held = state
        .lock()
        .instance_onboarding(instance)
        .map(|record| record.communication_limits.clone());
    assert!(
        matches!(held, Some(None)),
        "case \"unconfigured instance\": expected reading to leave the record unconfigured, \
         got {held:?}"
    );
}

/// An instance with no finished onboarding job has no configuration to read or to
/// write: both operations are HTTP 404 `ResourceNotFoundException`, and the refused
/// put stores nothing.
#[tokio::test]
async fn an_instance_that_is_not_onboarded_is_not_found() {
    let (router, _engine) = router_with_engine();
    let config = json!({
        "allChannelSubtypes": {"communicationLimitsList": [limit(8, 9, day())]}
    });

    // One onboarded and configured instance, so the 404s below are the lookup failing
    // and not the operations being broken.
    let configured = "configured-instance";
    onboard(&router, REGION, configured).await;
    let (status, body) = put_limits(&router, REGION, configured, &config).await;
    assert_eq!(
        status, 200,
        "case \"control instance\": expected HTTP 200, got {status} {body}"
    );

    // Well-formed `InstanceId`s this store holds no record for, including one that
    // only looks like the configured instance's id.
    for case in [
        "never-onboarded",
        "configured-instance-with-a-suffix",
        "x",
        "11111111-2222-3333-4444-555555555555",
    ] {
        let (status, body) = get_limits(&router, REGION, case).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404 reading an instance with no config, \
             got {status} {body}"
        );
        assert!(
            error_code(&body).ends_with("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {}",
            error_code(&body)
        );

        let (status, body) = put_limits(&router, REGION, case, &config).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404 configuring an instance with no config, \
             got {status} {body}"
        );
        assert!(
            error_code(&body).ends_with("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {}",
            error_code(&body)
        );

        // The refused put onboarded nothing, so the instance is still unknown.
        let (status, body) = get_limits(&router, REGION, case).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a refused put stores nothing, so the read is still a 404, \
             got {status} {body}"
        );
    }

    // An instance whose onboarding job has not finished yet is not configurable
    // either: the span the clock owes the job is never zero, so this request really
    // does reach the transient state.
    let starting = "still-onboarding-instance";
    let (status, body) = start_onboarding(&router, REGION, starting).await;
    assert_eq!(
        status, 200,
        "case \"job in progress\": expected onboarding to start, got {status} {body}"
    );
    let (status, body) = put_limits(&router, REGION, starting, &config).await;
    assert_eq!(
        status, 404,
        "case \"job in progress\": expected HTTP 404 while the job is unfinished, \
         got {status} {body}"
    );
    let (status, body) = get_limits(&router, REGION, starting).await;
    assert_eq!(
        status, 404,
        "case \"job in progress\": expected HTTP 404 while the job is unfinished, \
         got {status} {body}"
    );

    // Once the job is done the instance is configurable, and the put refused while it
    // was unfinished left nothing behind.
    tokio::time::sleep(ONBOARDING_JOB_DURATION + std::time::Duration::from_millis(150)).await;
    let stored = read_config(&router, REGION, starting, "job finished").await;
    assert_eq!(
        stored,
        Value::Null,
        "case \"job finished\": expected the refused put to have stored nothing, got {stored}"
    );

    // And the control instance still holds exactly what it was given.
    let stored = read_config(&router, REGION, configured, "control instance").await;
    assert_eq!(
        stored, config,
        "case \"control instance\": expected {config} to survive every refusal, got {stored}"
    );
}

/// A config outside `CommunicationLimit`'s modelled constraints is a bad request and
/// leaves the stored config alone, over a table of rows that each break one
/// constraint — including each published bound of `frequency`'s `@range` 1..30 and one
/// value past it, `maxCountPerRecipient`'s minimum, the `@length` maximum of
/// `communicationLimitsList` and the `CommunicationLimitTimeUnit` catalog.
#[tokio::test]
async fn a_config_outside_the_modelled_constraints_is_refused() {
    let router = router();
    let unit = day();
    let instance = "constraint-instance";
    onboard(&router, REGION, instance).await;

    // The config the instance holds throughout: no refusal may change it.
    let held = json!({
        "allChannelSubtypes": {"communicationLimitsList": [limit(6, 6, unit)]}
    });
    let (status, body) = put_limits(&router, REGION, instance, &held).await;
    assert_eq!(
        status, 200,
        "case \"the config held throughout\": expected HTTP 200, got {status} {body}"
    );

    let cases: Vec<(&str, Value)> = vec![
        (
            "frequency below its @range minimum of 1",
            json!({"allChannelSubtypes": {"communicationLimitsList": [limit(1, 0, unit)]}}),
        ),
        (
            "frequency above its @range maximum of 30",
            json!({"allChannelSubtypes": {"communicationLimitsList": [limit(1, 31, unit)]}}),
        ),
        (
            "maxCountPerRecipient below its @range minimum of 1",
            json!({"allChannelSubtypes": {"communicationLimitsList": [limit(0, 1, unit)]}}),
        ),
        (
            "three entries, one past communicationLimitsList's @length maximum of 2",
            json!({
                "allChannelSubtypes": {
                    "communicationLimitsList": [
                        limit(1, 1, unit), limit(2, 2, unit), limit(3, 3, unit)
                    ]
                }
            }),
        ),
        (
            "a unit outside CommunicationLimitTimeUnit's catalog",
            json!({"allChannelSubtypes": {"communicationLimitsList": [limit(1, 1, "WEEK")]}}),
        ),
        (
            "a lowercased catalog unit",
            json!({"allChannelSubtypes": {"communicationLimitsList": [limit(1, 1, "day")]}}),
        ),
    ];

    for (case, config) in cases {
        let (status, body) = put_limits(&router, REGION, instance, &config).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body}"
        );
        assert!(
            error_code(&body).ends_with("ValidationException"),
            "case {case:?}: expected ValidationException, got {}",
            error_code(&body)
        );

        let stored = read_config(&router, REGION, instance, case).await;
        assert_eq!(
            stored, held,
            "case {case:?}: expected a refusal to leave {held} stored, got {stored}"
        );
    }
}
