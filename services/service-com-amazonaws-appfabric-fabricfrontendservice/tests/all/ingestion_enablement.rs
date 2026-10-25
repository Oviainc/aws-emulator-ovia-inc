// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `StartIngestion` — the call that enables one stored
//! ingestion — driven over the wire (restJson1) through the same router the
//! emulator boots.
//!
//! Each test states a RULE over a TABLE of inputs rather than one recorded call:
//!
//! * a start moves the ingestion it names from `disabled` to `enabled` and
//!   PERSISTS the move, so `GetIngestion` and `ListIngestions` both report
//!   `enabled` afterwards, whichever `Identifier` form named the bundle and the
//!   ingestion;
//! * an ingestion that is not `disabled` has no such move to make and is refused
//!   with the modelled `ConflictException` (HTTP 409), unchanged;
//! * a target the store does not hold is refused with the modelled
//!   `ResourceNotFoundException` (HTTP 404) at either level of the chain, and
//!   nothing is written;
//! * start and stop alternate over any sequence of calls: each call decides from
//!   the STORED state, never from how many calls preceded it;
//! * a start touches exactly the ingestion it resolves to and leaves its
//!   siblings as they were.
//!
//! The harness (the router, `Answer`, `encode`, `arn_of`, the app-bundle and
//! ingestion calls) is the one `app_bundle_ingestion_lifecycle` already builds,
//! and the fixture chain (authorize → connect → ingest) the one
//! `app_authorization_and_ingestion_dependency` builds.

use serde_json::{json, Value};
use service_com_amazonaws_appfabric_fabricfrontendservice::state::{
    INGESTION_STATE_DISABLED, INGESTION_STATE_ENABLED,
};

use crate::app_authorization_and_ingestion_dependency::{
    bundle_with_ingestion, ingestion_state, stop_ingestion,
};
use crate::app_bundle_ingestion_lifecycle::{
    unknown_bundle_arn, unknown_ingestion_arn, unknown_uuid, uuid_of, Answer, Emulator, encode,
};

/// `POST /appbundles/{appBundleIdentifier}/ingestions/{ingestionIdentifier}/start`.
async fn start_ingestion(emulator: &Emulator, bundle: &str, ingestion: &str) -> Answer {
    emulator
        .call(
            "POST",
            &format!(
                "/appbundles/{}/ingestions/{}/start",
                encode(bundle),
                encode(ingestion)
            ),
            None,
        )
        .await
}

/// Either `Identifier` form of the same resource — the two the model's
/// `Identifier` shape accepts (`^arn:.+$` or a bare UUID).
fn as_identifier(arn: &str, by_arn: bool) -> String {
    if by_arn {
        arn.to_string()
    } else {
        uuid_of(arn)
    }
}

/// The `state` `ListIngestions` reports for one stored ingestion, read out of the
/// summary whose `arn` matches — the OTHER reader of the same record.
async fn listed_state(emulator: &Emulator, bundle: &str, ingestion_arn: &str) -> String {
    let listed = emulator.list_ingestions(bundle, "").await;
    listed
        .body
        .get("ingestions")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected ingestions in {}", listed.body))
        .iter()
        .find(|summary| summary.get("arn").and_then(Value::as_str) == Some(ingestion_arn))
        .and_then(|summary| summary.get("state"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected {ingestion_arn} in {}", listed.body))
        .to_string()
}

/// RULE: `StartIngestion` moves a `disabled` ingestion to `enabled` and persists
/// the move, so every later read of that record — `GetIngestion` and
/// `ListIngestions` alike — reports `enabled`; the success body is empty, so the
/// stored state is the whole of what the call reports.
///
/// The table spans applications the model documents, tenants, and both
/// `Identifier` forms at both levels of the chain.
#[tokio::test]
async fn starting_a_disabled_ingestion_enables_it_and_persists_it() {
    // (case, app, tenant, bundle by ARN, ingestion by ARN)
    let cases: Vec<(&str, &str, &str, bool, bool)> = vec![
        ("JIRA, both by ARN", "JIRA", "tenant-jira", true, true),
        (
            "OKTA, bundle by ARN, ingestion by uuid",
            "OKTA",
            "tenant-okta",
            true,
            false,
        ),
        (
            "DROPBOX, bundle by uuid, ingestion by ARN",
            "DROPBOX",
            "tenant-dropbox",
            false,
            true,
        ),
        (
            "SMARTSHEET, both by uuid",
            "SMARTSHEET",
            "tenant-smartsheet",
            false,
            false,
        ),
    ];

    for (case, app, tenant, bundle_by_arn, ingestion_by_arn) in cases {
        // A fresh store per row: one Region holds only one app bundle.
        let emulator = Emulator::new();
        let (bundle, ingestion) = bundle_with_ingestion(&emulator, app, tenant, case).await;

        let before = ingestion_state(&emulator, &bundle, &ingestion).await;
        assert_eq!(
            before, INGESTION_STATE_DISABLED,
            "case {case:?}: expected a new ingestion to start {INGESTION_STATE_DISABLED:?}, got {before:?}"
        );

        let answer = start_ingestion(
            &emulator,
            &as_identifier(&bundle, bundle_by_arn),
            &as_identifier(&ingestion, ingestion_by_arn),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "",
            "case {case:?}: expected no error envelope, got {}",
            answer.body
        );

        let after = ingestion_state(&emulator, &bundle, &ingestion).await;
        assert_eq!(
            after, INGESTION_STATE_ENABLED,
            "case {case:?}: expected GetIngestion to report {INGESTION_STATE_ENABLED:?}, got {after:?}"
        );
        let listed = listed_state(&emulator, &bundle, &ingestion).await;
        assert_eq!(
            listed, INGESTION_STATE_ENABLED,
            "case {case:?}: expected ListIngestions to report {INGESTION_STATE_ENABLED:?}, got {listed:?}"
        );
    }
}

/// RULE: an ingestion that is NOT `disabled` has no `disabled` → `enabled` move
/// to make, so the call is refused with the modelled `ConflictException`
/// (HTTP 409) naming the ingestion it resolved to, and the stored record is left
/// exactly as it was.
///
/// The table spans both `Identifier` forms and the number of refused retries, so
/// what decides the refusal is the stored state rather than the call count.
#[tokio::test]
async fn starting_an_ingestion_that_is_already_enabled_conflicts() {
    // (case, identifier form, retries after the first refusal)
    let cases: Vec<(&str, bool, usize)> = vec![
        ("by ARN, one refusal", true, 0),
        ("by bare uuid, one refusal", false, 0),
        ("by ARN, three refusals", true, 2),
    ];

    for (case, by_arn, retries) in cases {
        let emulator = Emulator::new();
        let (bundle, ingestion) =
            bundle_with_ingestion(&emulator, "ZENDESK", "tenant-conflict", case).await;
        let identifier = as_identifier(&ingestion, by_arn);

        let enabled = start_ingestion(&emulator, &bundle, &identifier).await;
        assert_eq!(
            enabled.status, 200,
            "case {case:?}: expected the first start to succeed, got {} with body {}",
            enabled.status, enabled.body
        );

        for attempt in 0..=retries {
            let refused = start_ingestion(&emulator, &bundle, &identifier).await;
            assert_eq!(
                refused.status, 409,
                "case {case:?} attempt {attempt}: expected HTTP 409, got {} with body {}",
                refused.status, refused.body
            );
            assert_eq!(
                refused.error_code(),
                "ConflictException",
                "case {case:?} attempt {attempt}: expected ConflictException, got {}",
                refused.body
            );
            assert_eq!(
                refused.body.get("resourceId").and_then(Value::as_str),
                Some(ingestion.as_str()),
                "case {case:?} attempt {attempt}: the conflict is about the stored ingestion, got {}",
                refused.body
            );
            assert_eq!(
                refused.body.get("resourceType").and_then(Value::as_str),
                Some("Ingestion"),
                "case {case:?} attempt {attempt}: expected the Ingestion resource type, got {}",
                refused.body
            );
            let state = ingestion_state(&emulator, &bundle, &ingestion).await;
            assert_eq!(
                state, INGESTION_STATE_ENABLED,
                "case {case:?} attempt {attempt}: a refused start must change nothing, got {state:?}"
            );
        }
    }
}

/// RULE: a start whose target the store does not hold is refused with the
/// modelled `ResourceNotFoundException` (HTTP 404) — at either level of the
/// chain and in either `Identifier` form — and nothing is written.
#[tokio::test]
async fn starting_a_target_that_is_not_stored_is_not_found() {
    let emulator = Emulator::new();
    let (bundle, ingestion) =
        bundle_with_ingestion(&emulator, "ASANA", "tenant-missing", "set-up").await;
    let unknown = unknown_uuid();

    // (case, bundle identifier, ingestion identifier, the missing resource kind)
    let cases: Vec<(&str, String, String, &str)> = vec![
        (
            "unknown ingestion uuid",
            bundle.clone(),
            unknown.clone(),
            "Ingestion",
        ),
        (
            "unknown ingestion arn",
            bundle.clone(),
            unknown_ingestion_arn(&uuid_of(&bundle), &unknown),
            "Ingestion",
        ),
        (
            "unknown bundle uuid",
            unknown.clone(),
            ingestion.clone(),
            "AppBundle",
        ),
        (
            "unknown bundle arn",
            unknown_bundle_arn(&unknown),
            ingestion.clone(),
            "AppBundle",
        ),
    ];

    for (case, bundle_identifier, ingestion_identifier, resource_type) in cases {
        let refused = start_ingestion(&emulator, &bundle_identifier, &ingestion_identifier).await;
        assert_eq!(
            refused.status, 404,
            "case {case:?}: expected HTTP 404, got {} with body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {}",
            refused.body
        );
        assert_eq!(
            refused.body.get("resourceType").and_then(Value::as_str),
            Some(resource_type),
            "case {case:?}: expected the {resource_type} resource type, got {}",
            refused.body
        );
    }

    let state = ingestion_state(&emulator, &bundle, &ingestion).await;
    assert_eq!(
        state, INGESTION_STATE_DISABLED,
        "a refused start must leave the stored ingestion as it was, got {state:?}"
    );
}

/// RULE (invariant): start and stop are inverse transitions over the model's two
/// `IngestionState` values, and EVERY call decides from the state the store
/// holds: a call that has the move to make succeeds (HTTP 200) and the store
/// reports the other state, a call that does not is refused HTTP 409 and the
/// store is unchanged — whatever order and however many calls precede it.
///
/// The sequence below walks both repeated successes and repeated refusals in
/// both directions, and the expected status of each step is COMPUTED from the
/// state machine rather than written down per step.
#[tokio::test]
async fn start_and_stop_decide_from_the_stored_state_over_any_sequence() {
    // `true` is a start, `false` is a stop.
    let sequence = [
        true, true, false, false, true, false, true, true, false, true,
    ];

    let emulator = Emulator::new();
    let (bundle, ingestion) =
        bundle_with_ingestion(&emulator, "GOOGLE", "tenant-sequence", "set-up").await;
    // Both forms are exercised above; this walk uses the bare UUID throughout.
    let identifier = uuid_of(&ingestion);
    let mut expected_state = INGESTION_STATE_DISABLED;

    for (step, is_start) in sequence.into_iter().enumerate() {
        let case = format!("step {step}: {}", if is_start { "start" } else { "stop" });
        let moves = if is_start {
            expected_state == INGESTION_STATE_DISABLED
        } else {
            expected_state == INGESTION_STATE_ENABLED
        };
        let answer = if is_start {
            start_ingestion(&emulator, &bundle, &identifier).await
        } else {
            stop_ingestion(&emulator, &bundle, &identifier).await
        };

        let expected_status = if moves { 200 } else { 409 };
        assert_eq!(
            answer.status, expected_status,
            "case {case:?}: expected HTTP {expected_status} from state {expected_state:?}, got {} with body {}",
            answer.status, answer.body
        );
        if moves {
            expected_state = if is_start {
                INGESTION_STATE_ENABLED
            } else {
                INGESTION_STATE_DISABLED
            };
        }
        let state = ingestion_state(&emulator, &bundle, &ingestion).await;
        assert_eq!(
            state, expected_state,
            "case {case:?}: expected the stored state to be {expected_state:?}, got {state:?}"
        );
    }
}

/// RULE: a start touches exactly the ingestion it resolves to — every sibling
/// under the same app bundle keeps the state it held.
///
/// The table spans which of several stored ingestions is started, so the rule is
/// asserted at the first, a middle and the last record of the bundle.
#[tokio::test]
async fn starting_one_ingestion_leaves_its_siblings_as_they_were() {
    // The (app, tenant) pairs this bundle's ingestions are created for: one
    // ingestion per pair, each with its own connected app authorization.
    let pairs = [
        ("SLACK", "tenant-sibling-a"),
        ("ZOOM", "tenant-sibling-b"),
        ("CISCO", "tenant-sibling-c"),
    ];

    for started in 0..pairs.len() {
        let emulator = Emulator::new();
        let bundle = emulator.fixture_bundle(0).await;
        let mut ingestions = Vec::new();
        for (app, tenant) in pairs {
            let created = emulator
                .create_ingestion(
                    &bundle,
                    json!({"app": app, "tenantId": tenant, "ingestionType": "auditLog"}),
                )
                .await;
            assert_eq!(
                created.status, 201,
                "sibling {app}/{tenant}: expected HTTP 201, got {} with body {}",
                created.status, created.body
            );
            ingestions.push(
                created
                    .body
                    .get("ingestion")
                    .and_then(|ingestion| ingestion.get("arn"))
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| panic!("expected ingestion.arn in {}", created.body))
                    .to_string(),
            );
        }

        let case = format!("started ingestion {started} of {}", pairs.len());
        let answer = start_ingestion(&emulator, &bundle, &ingestions[started]).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200, got {} with body {}",
            answer.status, answer.body
        );

        for (index, arn) in ingestions.iter().enumerate() {
            let expected = if index == started {
                INGESTION_STATE_ENABLED
            } else {
                INGESTION_STATE_DISABLED
            };
            let state = listed_state(&emulator, &bundle, arn).await;
            assert_eq!(
                state, expected,
                "case {case:?}: expected ingestion {index} to be {expected:?}, got {state:?}"
            );
        }
    }
}
