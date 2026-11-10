// SPDX-License-Identifier: CC0-1.0

//! What one `ConfigureLogsForPlaybackConfiguration` call reaches, end to end
//! over HTTP.
//!
//! The harness is the one `playback_configuration.rs` owns and the request
//! helpers are the ones `playback_configuration_logging.rs` owns, so every
//! assertion here goes through the router: a request is built, dispatched to
//! the mounted service, and the JSON response read back.
//!
//! The rule under test is the scope of that write: the call defines the log
//! configuration of the ONE playback configuration its
//! `PlaybackConfigurationName` names, so it leaves every other configuration's
//! log settings and every other member of the one it names exactly as they
//! stood; being idempotent, repeating a request leaves the same settings and
//! answers the same; a request that names no percentage does not say what to
//! log and is refused with nothing written; and a name no configuration is
//! stored under addresses nothing to configure, whether it was never created
//! or has been deleted.
//!
//! The tables span the class the rule generalises over: how many
//! configurations are stored and which of them is addressed, the settings each
//! one carries (the percentage at both documented bounds, each logging
//! strategy, the interaction-log settings), how many times one request is
//! repeated, and the forms a name can take.

use serde_json::{json, Value};

use crate::channel_lifecycle::{NOT_FOUND_CODE, NOT_FOUND_STATUS};
use crate::playback_configuration::{Emulator, Reply};
use crate::playback_configuration_logging::{
    configure_logs, expected_log_configuration, log_settings_table, LOGGING_STRATEGIES,
};

/// The log configuration a playback configuration reports before any
/// configure-logs call: the structure's own default, zero percent of sessions
/// with no strategy in force.
fn unconfigured() -> Value {
    json!({ "PercentEnabled": 0, "EnabledLoggingStrategies": [] })
}

/// The log configuration `name` reads back through `GetPlaybackConfiguration`.
async fn stored_log_configuration(emulator: &Emulator, name: &str) -> Value {
    emulator.get(name).await.get("/LogConfiguration").clone()
}

/// Creates `name` and asserts it was stored, so a later assertion about its log
/// settings is about a configuration that exists.
async fn create(emulator: &Emulator, name: &str) -> Reply {
    let created = emulator.put(&json!({ "Name": name })).await;
    assert_eq!(
        created.status, 200,
        "case {name:?}: expected the configuration to be created, got {} {:?}",
        created.status, created.body
    );
    created
}

#[tokio::test]
async fn a_configure_call_writes_only_the_configuration_it_names() {
    let emulator = Emulator::new();

    // Every row of the settings table gets its own configuration, all stored at
    // once, so each call is made with the others in the store: the row's
    // settings have to land on the row's configuration and on no other.
    let table = log_settings_table();
    let names: Vec<String> = table
        .iter()
        .map(|(case, _)| format!("scoped-{}", case.replace([' ', ','], "-")))
        .collect();
    for name in &names {
        create(&emulator, name).await;
    }

    for (index, (case, settings)) in table.iter().enumerate() {
        let configured = configure_logs(&emulator, &names[index], settings).await;
        assert_eq!(
            configured.status, 200,
            "case {case:?}: expected the configure call to be served, got {} {:?}",
            configured.status, configured.body
        );

        // After each call: the configurations already configured report their
        // own row's settings, and the ones not yet configured still report the
        // unconfigured default — so the write reached exactly one of them.
        for (other, (other_case, other_settings)) in table.iter().enumerate() {
            let expected = if other <= index {
                expected_log_configuration(other_settings)
            } else {
                unconfigured()
            };
            let got = stored_log_configuration(&emulator, &names[other]).await;
            assert_eq!(
                got, expected,
                "case {case:?}: after configuring it, expected {other_case:?} to report \
                 {expected:?}, got {got:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_configure_call_leaves_every_other_member_alone() {
    let emulator = Emulator::new();

    // The rows differ in which members the configuration carries, so what is
    // checked is that NONE of them moves, not just that the ones a bare
    // configuration has do not.
    for (case, mut request) in [
        ("a bare configuration", json!({})),
        (
            "the ad and content URLs",
            json!({
                "AdDecisionServerUrl": "https://ads.example.com/vast",
                "VideoContentSourceUrl": "https://origin.example.com/out/v1",
            }),
        ),
        (
            "a nested configuration and tags",
            json!({
                "AdDecisionServerUrl": "https://ads.example.com/vast",
                "InsertionMode": "STITCHED_ONLY",
                "PersonalizationThresholdSeconds": 30,
                "CdnConfiguration": { "AdSegmentUrlPrefix": "https://cdn.example.com/ads" },
                "tags": { "Env": "prod", "Team": "video" },
            }),
        ),
    ] {
        let name = format!("untouched-{}", case.replace([' ', ','], "-"));
        request
            .as_object_mut()
            .expect("the row is an object")
            .insert("Name".to_string(), json!(name));
        let created = emulator.put(&request).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected the configuration to be created, got {} {:?}",
            created.status, created.body
        );

        let before = emulator.get(&name).await.body;
        let settings = json!({
            "PercentEnabled": 42,
            "EnabledLoggingStrategies": [LOGGING_STRATEGIES[0]],
        });
        let configured = configure_logs(&emulator, &name, &settings).await;
        assert_eq!(
            configured.status, 200,
            "case {case:?}: expected the configure call to be served, got {} {:?}",
            configured.status, configured.body
        );
        let after = emulator.get(&name).await.body;

        // Only `LogConfiguration` differs: strip it from both reads and the
        // rest of the configuration has to compare equal member for member.
        let strip = |mut body: Value| -> Value {
            body.as_object_mut()
                .expect("an object body")
                .remove("LogConfiguration");
            body
        };
        let (stripped_before, stripped_after) = (strip(before), strip(after.clone()));
        assert_eq!(
            stripped_after, stripped_before,
            "case {case:?}: expected configuring logs to leave every other member as it stood \
             {stripped_before:?}, got {stripped_after:?}"
        );
        let expected = expected_log_configuration(&settings);
        assert_eq!(
            after.get("LogConfiguration"),
            Some(&expected),
            "case {case:?}: expected the log settings to be the ones written {expected:?}, got {:?}",
            after.get("LogConfiguration")
        );
    }
}

#[tokio::test]
async fn repeating_one_configure_request_reports_and_stores_the_same_settings() {
    let emulator = Emulator::new();

    // The operation is `@idempotent`
    // (`smithy_query(query="operation",operation="ConfigureLogsForPlaybackConfiguration")`
    // → `idempotent: true`), so sending one request again is the same request:
    // the answer and the stored settings do not move. The rows differ in the
    // settings sent and in how many times they are sent.
    for (case, settings) in log_settings_table() {
        for repeats in [2_usize, 3] {
            let name = format!("repeated-{}-{repeats}", case.replace([' ', ','], "-"));
            create(&emulator, &name).await;

            let mut answers: Vec<Value> = Vec::new();
            for attempt in 1..=repeats {
                let configured = configure_logs(&emulator, &name, &settings).await;
                assert_eq!(
                    configured.status, 200,
                    "case {case:?} attempt {attempt}: expected the repeat to be served, got {} {:?}",
                    configured.status, configured.body
                );
                answers.push(configured.body);
            }
            let first = &answers[0];
            for (attempt, answer) in answers.iter().enumerate().skip(1) {
                assert_eq!(
                    answer,
                    first,
                    "case {case:?} attempt {}: expected the repeat to answer what the first call \
                     answered {first:?}, got {answer:?}",
                    attempt + 1
                );
            }
            let expected = expected_log_configuration(&settings);
            let got = stored_log_configuration(&emulator, &name).await;
            assert_eq!(
                got, expected,
                "case {case:?}: expected {repeats} identical calls to leave {expected:?}, got {got:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_request_that_names_no_percentage_is_refused_and_writes_nothing() {
    let emulator = Emulator::new();

    // `PercentEnabled` is a required member
    // (`smithy_query(query="operation",operation="ConfigureLogsForPlaybackConfiguration")`
    // → `PercentEnabled { required: true }`), so a request that names no
    // percentage does not say what to log and is refused before anything is
    // written: the settings already in force stay in force. The rows differ in
    // what the request carries alongside the missing percentage, and in the
    // settings the refused call would have replaced.
    for (case, in_force, settings) in [
        (
            "nothing but the name, over settings that logged everything",
            json!({ "PercentEnabled": 100, "EnabledLoggingStrategies": LOGGING_STRATEGIES }),
            json!({}),
        ),
        (
            "a strategy but no percentage",
            json!({ "PercentEnabled": 50 }),
            json!({ "EnabledLoggingStrategies": [LOGGING_STRATEGIES[1]] }),
        ),
        (
            "the interaction logs but no percentage",
            json!({ "PercentEnabled": 1 }),
            json!({
                "AdsInteractionLog": { "ExcludeEventTypes": ["BEACON_FIRED"] },
                "ManifestServiceInteractionLog": { "ExcludeEventTypes": ["GENERATED_MANIFEST"] },
            }),
        ),
    ] {
        let name = format!("no-percentage-{}", case.replace([' ', ','], "-"));
        create(&emulator, &name).await;
        assert_eq!(
            configure_logs(&emulator, &name, &in_force).await.status,
            200,
            "case {case:?}: expected the settings under test to be written"
        );

        let refused = configure_logs(&emulator, &name, &settings).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected the missing percentage to be refused, got {} {:?}",
            refused.status, refused.body
        );
        let expected = expected_log_configuration(&in_force);
        let got = stored_log_configuration(&emulator, &name).await;
        assert_eq!(
            got, expected,
            "case {case:?}: expected the refused call to leave {expected:?} in force, got {got:?}"
        );
    }
}

#[tokio::test]
async fn a_name_no_configuration_is_stored_under_is_refused() {
    let emulator = Emulator::new();
    let settings = json!({ "PercentEnabled": 10, "EnabledLoggingStrategies": LOGGING_STRATEGIES });

    // A configuration is stored under its `Name`, so the names that address
    // nothing are the ones nothing was ever stored under AND the ones whose
    // configuration has since been deleted. The rows span several forms of
    // name, including one that only differs from a stored name in case.
    create(&emulator, "stored-elsewhere").await;
    let deleted = "deleted-configuration";
    create(&emulator, deleted).await;
    assert_eq!(
        configure_logs(&emulator, deleted, &settings).await.status,
        200,
        "expected the configuration to accept log settings while it was stored"
    );
    let removed = emulator
        .request(
            "DELETE",
            &format!("/playbackConfiguration/{deleted}"),
            None,
        )
        .await;
    assert_eq!(
        removed.status, 204,
        "expected the configuration to be deleted, got {} {:?}",
        removed.status, removed.body
    );

    for (case, name) in [
        ("never created", "never-created"),
        ("deleted since it was configured", deleted),
        ("a stored name in another case", "STORED-ELSEWHERE"),
        ("a stored name with a suffix", "stored-elsewhere-2"),
        ("a prefix of a stored name", "stored"),
    ] {
        let refused = configure_logs(&emulator, name, &settings).await;
        assert_eq!(
            refused.status, NOT_FOUND_STATUS,
            "case {case:?}: expected the request to be refused as not found, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code, NOT_FOUND_CODE,
            "case {case:?}: expected {NOT_FOUND_CODE}, got {:?}",
            refused.error_code
        );
        assert_eq!(
            refused.string("/message"),
            format!("PlaybackConfiguration {name} not found"),
            "case {case:?}: expected the refusal to name the configuration it could not find, \
             got {:?}",
            refused.body
        );
    }

    // The refused calls stored nothing: neither a configuration under a name
    // that addressed nothing, nor log settings on the one that does exist.
    let listed = emulator.list(Some(100), None).await;
    let names: Vec<Value> = listed
        .get("/Items")
        .as_array()
        .expect("Items is a list")
        .iter()
        .map(|item| item.get("Name").cloned().unwrap_or(Value::Null))
        .collect();
    assert_eq!(
        names,
        vec![json!("stored-elsewhere")],
        "expected the refused calls to have created nothing, got {names:?}"
    );
    let got = stored_log_configuration(&emulator, "stored-elsewhere").await;
    assert_eq!(
        got,
        unconfigured(),
        "expected the configuration nothing configured to report no logging, got {got:?}"
    );
}
