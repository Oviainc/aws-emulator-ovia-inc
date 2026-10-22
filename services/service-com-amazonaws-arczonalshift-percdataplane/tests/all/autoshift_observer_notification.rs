// SPDX-License-Identifier: CC0-1.0

//! The account-wide autoshift observer notification setting, driven end to end.
//!
//! The rules under test, each asserted over a SET of inputs rather than one request:
//!
//! * the status `UpdateAutoshiftObserverNotificationStatus` writes is PERSISTED, so
//!   `GetAutoshiftObserverNotificationStatus` reports the value the last update set —
//!   for EVERY value the modelled enum admits, in any order, and repeating an update
//!   leaves the same value (the operation is `@idempotent`);
//! * `GetAutoshiftObserverNotificationStatus` is `@readonly`: reading does not change
//!   what a later caller sees, whatever the setting is;
//! * the setting is account-wide — it names no resource, and writing it moves no
//!   resource's `zonalAutoshiftStatus`;
//! * a `status` outside the modelled enum is refused 400 `ValidationException` and
//!   leaves the stored setting as it was.

use super::zonal_shift_lifecycle::{call, emulator, encoded, error_code, seeded};
use service_com_amazonaws_arczonalshift_percdataplane::ops::types::AUTOSHIFT_OBSERVER_NOTIFICATION_STATUS_VALUES;
use service_com_amazonaws_arczonalshift_percdataplane::state::{
    AutoshiftObserverNotificationStatus, DEFAULT_AUTOSHIFT_OBSERVER_NOTIFICATION_STATUS,
};

/// The one URI both operations are bound to (`GET` and `PUT`
/// `/autoshift-observer-notification`, from `smithy_query(query="operation",
/// operation="GetAutoshiftObserverNotificationStatus,UpdateAutoshiftObserverNotificationStatus")`).
const URI: &str = "/autoshift-observer-notification";

/// The sequences of settings a case walks. The values come from the model's
/// `AutoshiftObserverNotificationStatus` enum (the exported
/// `AUTOSHIFT_OBSERVER_NOTIFICATION_STATUS_VALUES`); the rows differ in ORDER,
/// COUNT and repetition, so what is pinned is "the last write wins and is kept"
/// rather than one transition.
const SEQUENCES: &[&[usize]] = &[
    &[0],
    &[1],
    &[0, 1],
    &[1, 0],
    &[1, 1, 0, 0, 1],
    &[0, 1, 0, 1, 0],
];

/// `status` values the modelled enum does not admit, as `(value, class)`.
const INVALID_STATUSES: &[(&str, &str)] = &[
    ("enabled", "the right word in the wrong case"),
    ("Disabled", "mixed case"),
    ("ENABLE", "a prefix of a value the enum admits"),
    ("ENABLED ", "a value the enum admits with trailing space"),
    ("", "the empty string"),
    ("SOMETHING_ELSE", "a value the enum never had"),
];

fn update_body(status: &str) -> String {
    serde_json::json!({ "status": status }).to_string()
}

/// Read the setting back and hand over the value reported, failing the case if the
/// call is not a 200 carrying the required `status` member.
async fn get_status(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
) -> String {
    let (status_code, body) = call(router, "GET", URI, "").await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: GetAutoshiftObserverNotificationStatus answers 200, \
         got {status_code} {body}"
    );
    body["status"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: status is required, got {body}"))
        .to_string()
}

#[tokio::test]
async fn the_status_an_update_sets_is_reported_by_the_next_read() {
    for sequence in SEQUENCES {
        // One fresh emulator per row, so a row starts from the default rather than
        // from whatever the previous row left behind.
        let (router, _engine) = emulator();
        for index in sequence.iter() {
            let wanted = AUTOSHIFT_OBSERVER_NOTIFICATION_STATUS_VALUES[*index];
            let case = format!("sequence {sequence:?} -> {wanted}");
            let (status_code, body) = call(&router, "PUT", URI, &update_body(wanted)).await;
            assert_eq!(
                status_code, 200,
                "case {case:?}: UpdateAutoshiftObserverNotificationStatus answers 200, \
                 got {status_code} {body}"
            );
            assert_eq!(
                body["status"].as_str(),
                Some(wanted),
                "case {case:?}: the update reports the status now in effect, got {body}"
            );

            // The round trip: the value the update wrote is what a later caller reads.
            let read_back = get_status(&router, &case).await;
            assert_eq!(
                read_back, wanted,
                "case {case:?}: the stored status is reported, got {read_back}"
            );
        }
    }
}

#[tokio::test]
async fn an_account_that_never_set_a_status_reads_the_default() {
    let (router, _engine) = emulator();
    let first = get_status(&router, "a store nobody has written").await;
    assert_eq!(
        first,
        DEFAULT_AUTOSHIFT_OBSERVER_NOTIFICATION_STATUS.as_wire(),
        "an unset account reports the default, got {first}"
    );
    assert!(
        AUTOSHIFT_OBSERVER_NOTIFICATION_STATUS_VALUES.contains(&first.as_str()),
        "the default is a value the model admits, got {first}"
    );
}

#[tokio::test]
async fn reading_the_status_never_changes_it() {
    // `@readonly`: for EVERY setting, including the default, repeated reads report
    // the same value, so a read changes nothing a later caller sees.
    let (router, _engine) = emulator();
    let mut expected = DEFAULT_AUTOSHIFT_OBSERVER_NOTIFICATION_STATUS
        .as_wire()
        .to_string();
    for round in 0..3 {
        let case = format!("round {round} of the default setting");
        assert_eq!(
            get_status(&router, &case).await,
            expected,
            "case {case:?}: a read leaves the setting alone"
        );
    }
    for wanted in AUTOSHIFT_OBSERVER_NOTIFICATION_STATUS_VALUES {
        let (status_code, body) = call(&router, "PUT", URI, &update_body(wanted)).await;
        assert_eq!(
            status_code, 200,
            "setting {wanted}: the update answers 200, got {status_code} {body}"
        );
        expected = (*wanted).to_string();
        for round in 0..3 {
            let case = format!("round {round} of {wanted}");
            assert_eq!(
                get_status(&router, &case).await,
                expected,
                "case {case:?}: a read leaves the setting alone"
            );
        }
    }
}

#[tokio::test]
async fn a_status_the_model_does_not_admit_is_refused_and_changes_nothing() {
    let (router, _engine) = emulator();
    // Start from a value a caller set, so a refusal that reset the setting would be
    // visible as a change away from it rather than hidden by the default.
    let held = AUTOSHIFT_OBSERVER_NOTIFICATION_STATUS_VALUES[1];
    let (status_code, body) = call(&router, "PUT", URI, &update_body(held)).await;
    assert_eq!(
        status_code, 200,
        "the setting is established first, got {status_code} {body}"
    );

    let mut payloads: Vec<(String, String)> = INVALID_STATUSES
        .iter()
        .map(|(value, class)| (update_body(value), (*class).to_string()))
        .collect();
    // A required member the model declares, left out entirely.
    payloads.push(("{}".to_string(), "status omitted".to_string()));

    for (payload, class) in &payloads {
        let (status_code, body) = call(&router, "PUT", URI, payload).await;
        assert_eq!(
            status_code, 400,
            "case {class:?}: an unmodelled status is refused 400, got {status_code} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {class:?}: expected ValidationException, got {body}"
        );
        let read_back = get_status(&router, class).await;
        assert_eq!(
            read_back, held,
            "case {class:?}: a refused update leaves the stored status at {held}, \
             got {read_back}"
        );
    }
}

#[tokio::test]
async fn the_setting_is_account_wide_and_moves_no_resource() {
    // The operations name no resource, so writing the account setting must not touch
    // a managed resource's own `zonalAutoshiftStatus` — a different enum with the
    // same two values.
    let (router, engine) = emulator();
    let before: Vec<(String, Option<String>)> = read_resource_statuses(&router, &engine).await;
    for wanted in AUTOSHIFT_OBSERVER_NOTIFICATION_STATUS_VALUES {
        let (status_code, body) = call(&router, "PUT", URI, &update_body(wanted)).await;
        assert_eq!(
            status_code, 200,
            "setting {wanted}: the update answers 200, got {status_code} {body}"
        );
        let after = read_resource_statuses(&router, &engine).await;
        assert_eq!(
            after, before,
            "setting {wanted}: no resource's zonalAutoshiftStatus moves, got {after:?}"
        );
    }

    // And the parse of the wire value is total over the enum: every value the model
    // admits is a status this service holds.
    for wanted in AUTOSHIFT_OBSERVER_NOTIFICATION_STATUS_VALUES {
        assert!(
            AutoshiftObserverNotificationStatus::parse(wanted).is_some(),
            "setting {wanted}: the model's value must parse"
        );
    }
}

/// Each seeded managed resource's `zonalAutoshiftStatus`, read over the wire.
async fn read_resource_statuses(
    router: &smithy_server_runtime::MultiServiceRouter,
    engine: &engine::InMemoryEngine,
) -> Vec<(String, Option<String>)> {
    let mut statuses = Vec::new();
    for (resource, _zones) in seeded(engine) {
        let (status_code, body) = call(
            router,
            "GET",
            &format!("/managedresources/{}", encoded(&resource)),
            "",
        )
        .await;
        assert_eq!(
            status_code, 200,
            "{resource}: GetManagedResource answers 200, got {status_code} {body}"
        );
        statuses.push((
            resource,
            body["zonalAutoshiftStatus"].as_str().map(str::to_string),
        ));
    }
    statuses
}
