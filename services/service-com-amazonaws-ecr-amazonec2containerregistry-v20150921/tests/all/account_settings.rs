// SPDX-License-Identifier: CC0-1.0

//! The ECR account settings, end to end over the wire.
//!
//! The rule under test: an account setting is a named value of this registry that
//! `PutAccountSetting` stores and `GetAccountSetting` reports, unchanged until the next
//! put, with a name the registry does not serve and a value the named setting does not
//! take both refused as an invalid parameter and neither of them changing stored state.
//!
//! Every case goes through the router, so it exercises routing, deserialization, the
//! handler, the persisted state and serialization — the same path a client takes.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_ecr_amazonec2containerregistry_v20150921::state::{
    service_state, AccountSettingSpec, ACCOUNT_SETTINGS,
};
use service_com_amazonaws_ecr_amazonec2containerregistry_v20150921::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator, Answer};

/// A top-level string member of an answered body, or the empty string when it is absent.
fn member<'a>(answer: &'a Answer, name: &str) -> &'a str {
    answer.body[name].as_str().unwrap_or_default()
}

/// A successful call, or a panic naming the case and what came back instead. Unlike
/// [`crate::wire::ok`] this keeps the whole [`Answer`], which these cases assert on.
async fn ok(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Answer {
    let answer = call(router, operation, body).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200 from {operation}, got {} with {:?}",
        answer.status, answer.body
    );
    answer
}

/// `GetAccountSetting` on `name`.
async fn get(router: &MultiServiceRouter, case: &str, name: &str) -> Answer {
    ok(router, case, "GetAccountSetting", &json!({"name": name})).await
}

/// `PutAccountSetting` setting `name` to `value`.
async fn put(router: &MultiServiceRouter, case: &str, name: &str, value: &str) -> Answer {
    ok(
        router,
        case,
        "PutAccountSetting",
        &json!({"name": name, "value": value}),
    )
    .await
}

/// The value to set a setting to when the case just needs it changed: one that differs
/// from its default where the setting has one, so the change is observable.
fn value_to_set(spec: &AccountSettingSpec) -> &'static str {
    spec.settable_values
        .iter()
        .copied()
        .find(|value| *value != spec.default_value)
        .unwrap_or(spec.settable_values[0])
}

/// A value the setting does not take, chosen from another setting's value set so the row
/// is a real value of the API rather than nonsense.
fn value_another_setting_takes(spec: &AccountSettingSpec) -> &'static str {
    ACCOUNT_SETTINGS
        .iter()
        .flat_map(|other| other.settable_values.iter().copied())
        .find(|value| !spec.settable_values.contains(value))
        .expect("the catalogue has at least two settings with different value sets")
}

/// Every setting the registry serves round-trips: what a put stored is what the next get
/// reports, for every value that setting takes, and a repeated put of the same value
/// leaves it there.
///
/// The table is the whole catalogue (`state::ACCOUNT_SETTINGS`, built from the model's
/// documentation of `PutAccountSettingRequest$name` and `$value`) crossed with every
/// value each setting takes, so it spans settings with one settable value and settings
/// with two, and the setting names the issue did not cite as well as the one it did.
#[tokio::test]
async fn a_put_account_setting_is_what_the_next_get_reports() {
    let router = emulator();

    for spec in ACCOUNT_SETTINGS {
        let name = spec.name;
        for value in spec.settable_values {
            let case = format!("{name} = {value}");

            // The put reports the pair it stored.
            let stored = put(&router, &case, name, value).await;
            assert_eq!(
                member(&stored, "name"),
                name,
                "case {case:?}: PutAccountSetting must report the setting it set, got {:?}",
                stored.body
            );
            assert_eq!(
                member(&stored, "value"),
                *value,
                "case {case:?}: PutAccountSetting must report the value it stored, got {:?}",
                stored.body
            );

            // And the get reports the same thing, as many times as it is asked: a read
            // never changes what a later caller sees.
            for read in 1..=2 {
                let reported = get(&router, &case, name).await;
                assert_eq!(
                    member(&reported, "name"),
                    name,
                    "case {case:?}: read {read} must report the setting asked for, got {:?}",
                    reported.body
                );
                assert_eq!(
                    member(&reported, "value"),
                    *value,
                    "case {case:?}: read {read} must report the stored value, got {:?}",
                    reported.body
                );
            }

            // Setting the same value again is idempotent.
            let again = put(&router, &case, name, value).await;
            assert_eq!(
                member(&again, "value"),
                *value,
                "case {case:?}: a repeated put must leave the stored value alone, got {:?}",
                again.body
            );
        }
    }
}

/// Every accepted put LANDS IN THE STORE, under the setting's own name.
///
/// This case holds the engine rather than using the shared harness, because for a setting
/// whose only settable value equals its default the response alone cannot tell a stored
/// value from a default — only the store can.
#[tokio::test]
async fn an_accepted_put_lands_in_the_stores_own_account_settings_map() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));

    for spec in ACCOUNT_SETTINGS {
        for value in spec.settable_values {
            let case = format!("{} = {value}", spec.name);
            put(&router, &case, spec.name, value).await;
            let stored = service_state(&*engine)
                .expect("the ecr store is reachable from the shared engine")
                .lock()
                .account_settings
                .get(spec.name)
                .cloned();
            assert_eq!(
                stored.as_deref(),
                Some(*value),
                "case {case:?}: the put must be persisted under the setting's name, \
                 the store holds {stored:?}"
            );
        }
    }
}

/// A setting nobody has set reports its documented default, and that default is a value
/// the API knows — every setting is readable before any put, and reading one does not
/// fix it in place or disturb its neighbours.
#[tokio::test]
async fn an_unset_setting_reports_its_default_and_reading_it_changes_nothing() {
    let router = emulator();

    for spec in ACCOUNT_SETTINGS {
        let case = format!("{} unset", spec.name);
        let reported = get(&router, &case, spec.name).await;
        assert_eq!(
            member(&reported, "value"),
            spec.default_value,
            "case {case:?}: an unset setting must report its default, got {:?}",
            reported.body
        );
    }

    // Each setting is independent: changing one leaves every other where it was.
    for spec in ACCOUNT_SETTINGS {
        let case = format!("{} set, the others untouched", spec.name);
        put(&router, &case, spec.name, value_to_set(spec)).await;

        for other in ACCOUNT_SETTINGS {
            if other.name == spec.name {
                continue;
            }
            let reported = get(&router, &case, other.name).await;
            let already_set = ACCOUNT_SETTINGS
                .iter()
                .take_while(|earlier| earlier.name != spec.name)
                .any(|earlier| earlier.name == other.name);
            let expected = if already_set {
                value_to_set(other)
            } else {
                other.default_value
            };
            assert_eq!(
                member(&reported, "value"),
                expected,
                "case {case:?}: {} must still report {expected}, got {:?}",
                other.name,
                reported.body
            );
        }
    }
}

/// A name this registry does not serve is refused by both operations with the modelled
/// `InvalidParameterException`, and nothing is stored for it.
///
/// The rows differ along the shape of the name: unknown, differently cased, a prefix and
/// a suffix of a real one, and the longest and shortest the model's `@length(1..64)`
/// admits — so a name is matched, never guessed at.
#[tokio::test]
async fn a_setting_name_this_registry_does_not_serve_is_refused() {
    let router = emulator();
    // Every setting is first set to a value of its own, so "unchanged" below is a value
    // the refused call could not have produced and not merely the default.
    for spec in ACCOUNT_SETTINGS {
        put(&router, spec.name, spec.name, value_to_set(spec)).await;
    }
    let longest = "A".repeat(64);
    let cases: Vec<(&str, String)> = vec![
        ("a name of no setting", "NOT_A_SETTING".to_string()),
        (
            "a real name in lower case",
            "basic_scan_type_version".to_string(),
        ),
        (
            "a prefix of a real name",
            "BASIC_SCAN_TYPE".to_string(),
        ),
        (
            "a real name with a suffix",
            "BLOB_MOUNTING_V2".to_string(),
        ),
        ("the shortest name the model admits", "A".to_string()),
        ("the longest name the model admits", longest),
    ];

    for (case, name) in cases {
        for operation in ["GetAccountSetting", "PutAccountSetting"] {
            let answer = call(
                &router,
                operation,
                &json!({"name": name, "value": "ENABLED"}),
            )
            .await;
            assert_eq!(
                answer.status, 400,
                "case {case:?}: expected HTTP 400 from {operation}, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "InvalidParameterException",
                "case {case:?}: expected InvalidParameterException from {operation}, got {:?}",
                answer.body
            );
        }

        // Nothing was stored under the refused name, and no setting the registry does
        // serve moved: each still reports what was set before the refused calls.
        for spec in ACCOUNT_SETTINGS {
            let untouched = get(&router, case, spec.name).await;
            assert_eq!(
                member(&untouched, "value"),
                value_to_set(spec),
                "case {case:?}: a refused name must leave {} alone, got {:?}",
                spec.name,
                untouched.body
            );
        }
    }

    // One character past the model's `@length(min: 1, max: 64)` on `name` is refused at
    // the boundary, before any handler sees it.
    let too_long = "A".repeat(65);
    for operation in ["GetAccountSetting", "PutAccountSetting"] {
        let answer = call(
            &router,
            operation,
            &json!({"name": too_long, "value": "ENABLED"}),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "a 65-character name must be refused by {operation}, got {} with {:?}",
            answer.status, answer.body
        );
    }
}

/// A value the named setting does not take is refused, and the setting keeps the value it
/// had.
///
/// One row per setting, each sending a value that belongs to a different setting, plus
/// the shapes a value can be wrong in.
#[tokio::test]
async fn a_value_the_setting_does_not_take_is_refused_and_changes_nothing() {
    let router = emulator();

    for spec in ACCOUNT_SETTINGS {
        let accepted = spec.settable_values[0];
        put(&router, spec.name, spec.name, accepted).await;

        let other = value_another_setting_takes(spec);
        let lowered = accepted.to_lowercase();
        let padded = format!(" {accepted} ");
        let rejected: Vec<(&str, &str)> = vec![
            ("a value of another setting", other),
            ("the accepted value in lower case", lowered.as_str()),
            ("the accepted value with spaces", padded.as_str()),
            ("the empty value", ""),
        ];

        for (why, value) in rejected {
            let case = format!("{}: {why}", spec.name);
            let answer = call(
                &router,
                "PutAccountSetting",
                &json!({"name": spec.name, "value": value}),
            )
            .await;
            assert_eq!(
                answer.status, 400,
                "case {case:?}: expected HTTP 400, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "InvalidParameterException",
                "case {case:?}: expected InvalidParameterException, got {:?}",
                answer.body
            );
            // The refusal says which setting and which value it refused: the wire
            // wording is pinned here, not borrowed from the implementation.
            assert_eq!(
                member(&answer, "message"),
                format!("You can't set '{}' to '{value}' in this region.", spec.name),
                "case {case:?}: the refusal must name the setting and the value, got {:?}",
                answer.body
            );

            let reported = get(&router, &case, spec.name).await;
            assert_eq!(
                member(&reported, "value"),
                accepted,
                "case {case:?}: a refused put must leave the stored value alone, got {:?}",
                reported.body
            );
        }
    }
}
