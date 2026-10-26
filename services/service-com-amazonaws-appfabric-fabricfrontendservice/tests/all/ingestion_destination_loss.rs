// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what losing a destination association does to the
//! INGESTION that owned it, driven over the wire (restJson1) through the same
//! router the emulator boots.
//!
//! Each test states a RULE over a TABLE of inputs rather than one recorded call:
//!
//! * an ingestion that loses a destination association while it is `enabled` has
//!   a disable owed to it by the clock: it still reads `enabled` straight after
//!   the delete, and from the instant the owed disable falls due every reader of
//!   the record — `GetIngestion` and `ListIngestions` alike — reports `disabled`;
//! * an ingestion that was not `enabled` when the destination went has no
//!   delivery to fail, so nothing is owed and it keeps the state it held;
//! * the state the clock owes is the state every OTHER call decides from:
//!   `DeleteIngestion` refuses the ingestion until the disable is due and
//!   succeeds afterwards with no explicit stop call, `StopIngestion` has nothing
//!   left to stop, and `StartIngestion` enables it again and discards the owed
//!   disable.
//!
//! The harness (the router, `Answer`, `encode`, `arn_of`, `uuid_of`) is the one
//! `app_bundle_ingestion_lifecycle` builds, the fixture chain (authorize →
//! connect → ingest) the one `app_authorization_and_ingestion_dependency`
//! builds, and the start/stop and read helpers are those suites' own.

use serde_json::Value;
use service_com_amazonaws_appfabric_fabricfrontendservice::state::{
    INGESTION_STATE_DISABLED, INGESTION_STATE_ENABLED,
};

use crate::app_authorization_and_ingestion_dependency::{
    bundle_with_ingestion, create_destination, ingestion_state, stop_ingestion,
};
use crate::app_bundle_ingestion_lifecycle::{arn_of, encode, uuid_of, Answer, Emulator};
use crate::ingestion_enablement::{listed_state, start_ingestion};

/// Comfortably longer than the span the emulator makes the documented disable
/// fall due after (`state::INGESTION_DISABLE_AFTER_DESTINATION_LOSS`, 500 ms), so
/// a read after it is a read of a transition the clock already owes.
const PAST_THE_DISABLE_SPAN: std::time::Duration = std::time::Duration::from_millis(900);

/// `DELETE /appbundles/{bundle}/ingestions/{ingestion}/ingestiondestinations/{destination}`.
async fn delete_destination(
    emulator: &Emulator,
    bundle: &str,
    ingestion: &str,
    destination: &str,
) -> Answer {
    emulator
        .call(
            "DELETE",
            &format!(
                "/appbundles/{}/ingestions/{}/ingestiondestinations/{}",
                encode(bundle),
                encode(ingestion),
                encode(destination)
            ),
            None,
        )
        .await
}

/// The `count` destinations this test puts under one ingestion, as their ARNs:
/// one per S3 bucket, so each row of a table can delete some of several.
async fn destinations(
    emulator: &Emulator,
    bundle: &str,
    ingestion: &str,
    count: usize,
    case: &str,
) -> Vec<String> {
    let mut created = Vec::with_capacity(count);
    for index in 0..count {
        let answer =
            create_destination(emulator, bundle, ingestion, &format!("bucket-{index}")).await;
        assert_eq!(
            answer.status, 201,
            "case {case:?}: expected destination {index} to be created, got {} with body {}",
            answer.status, answer.body
        );
        created.push(arn_of(&answer.body, "ingestionDestination"));
    }
    created
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

/// The `updatedAt` `GetIngestion` reports for one stored ingestion, as the epoch
/// seconds the wire carries.
async fn ingestion_updated_at(emulator: &Emulator, bundle: &str, ingestion: &str) -> f64 {
    let read = emulator.get_ingestion(bundle, ingestion).await;
    read.body
        .get("ingestion")
        .and_then(|ingestion| ingestion.get("updatedAt"))
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("expected ingestion.updatedAt in {}", read.body))
}

/// RULE: an ingestion that loses a destination association while it is `enabled`
/// fails and the service disables it shortly after; an ingestion in any other
/// state has no delivery to fail and keeps the state it held. The transition is
/// the CLOCK's, so the span is non-zero (a read straight after the delete still
/// sees `enabled`) and, once due, EVERY reader of the record reports it.
///
/// The table spans the state the ingestion was in, the `Identifier` form the
/// delete named the destination by, how many destinations the ingestion held and
/// how many of them the delete removed.
#[tokio::test]
async fn losing_a_destination_while_enabled_leaves_the_ingestion_disabled() {
    // (case, app, tenant, enabled before the delete, destination by ARN,
    //  destinations created, destinations deleted)
    let cases: Vec<(&str, &str, &str, bool, bool, usize, usize)> = vec![
        (
            "enabled, its only destination, by ARN",
            "SLACK",
            "tenant-slack",
            true,
            true,
            1,
            1,
        ),
        (
            "enabled, its only destination, by bare uuid",
            "ZOOM",
            "tenant-zoom",
            true,
            false,
            1,
            1,
        ),
        (
            "enabled, one of two destinations",
            "JIRA",
            "tenant-jira",
            true,
            true,
            2,
            1,
        ),
        (
            "enabled, both of two destinations",
            "OKTA",
            "tenant-okta",
            true,
            false,
            2,
            2,
        ),
        (
            "never started, its only destination",
            "ASANA",
            "tenant-asana",
            false,
            true,
            1,
            1,
        ),
    ];

    for (case, app, tenant, enabled, by_arn, created, deleted) in cases {
        // A fresh store per row: one Region holds only one app bundle.
        let emulator = Emulator::new();
        let (bundle, ingestion) = bundle_with_ingestion(&emulator, app, tenant, case).await;
        let targets = destinations(&emulator, &bundle, &ingestion, created, case).await;
        if enabled {
            let started = start_ingestion(&emulator, &bundle, &ingestion).await;
            assert_eq!(
                started.status, 200,
                "case {case:?}: expected the ingestion to start, got {} with body {}",
                started.status, started.body
            );
        }

        let expected_before = if enabled {
            INGESTION_STATE_ENABLED
        } else {
            INGESTION_STATE_DISABLED
        };
        let before = ingestion_state(&emulator, &bundle, &ingestion).await;
        assert_eq!(
            before, expected_before,
            "case {case:?}: expected the ingestion to be {expected_before:?} before the delete, got {before:?}"
        );
        let updated_before = ingestion_updated_at(&emulator, &bundle, &ingestion).await;

        for (index, target) in targets.iter().take(deleted).enumerate() {
            let removed = delete_destination(
                &emulator,
                &bundle,
                &ingestion,
                &as_identifier(target, by_arn),
            )
            .await;
            assert_eq!(
                removed.status, 204,
                "case {case:?}: expected destination {index} to be deleted, got {} with body {}",
                removed.status, removed.body
            );
        }

        // The span the clock owes is non-zero, so a caller reading straight
        // after the delete still sees the state the delete left behind.
        let immediately = ingestion_state(&emulator, &bundle, &ingestion).await;
        assert_eq!(
            immediately, expected_before,
            "case {case:?}: expected {expected_before:?} straight after the delete, got {immediately:?}"
        );

        tokio::time::sleep(PAST_THE_DISABLE_SPAN).await;

        // Whatever the state was, it is `disabled` once the clock has made the
        // documented disable due — and both readers of the record agree.
        let after = ingestion_state(&emulator, &bundle, &ingestion).await;
        assert_eq!(
            after, INGESTION_STATE_DISABLED,
            "case {case:?}: expected GetIngestion to report {INGESTION_STATE_DISABLED:?} once the disable is due, got {after:?}"
        );
        let listed = listed_state(&emulator, &bundle, &ingestion).await;
        assert_eq!(
            listed, INGESTION_STATE_DISABLED,
            "case {case:?}: expected ListIngestions to report {INGESTION_STATE_DISABLED:?}, got {listed:?}"
        );
        let updated_after = ingestion_updated_at(&emulator, &bundle, &ingestion).await;
        if enabled {
            // The disable IS a change to the record, so it dates `updatedAt`.
            assert!(
                updated_after > updated_before,
                "case {case:?}: expected updatedAt to move with the disable, got {updated_after:?} after {updated_before:?}"
            );
        } else {
            // Nothing changed the record, so nothing dated it either.
            assert!(
                (updated_after - updated_before).abs() < 1e-6,
                "case {case:?}: expected updatedAt to be left alone, got {updated_after:?} after {updated_before:?}"
            );
        }
    }
}

/// RULE: the state the clock owes an ingestion is the state every other call
/// decides from, so an ingestion whose destination went while it was `enabled`
/// needs no explicit stop before it can be deleted: `DeleteIngestion` refuses it
/// with the modelled `ValidationException` (HTTP 400) naming the state it is
/// still in, and succeeds with the modelled empty HTTP 204 once the disable is
/// due — after which the ingestion is gone.
///
/// The table spans how many destinations the ingestion held (the delete of the
/// ingestion also requires every one of them to be gone) and the `Identifier`
/// form each call names its target by.
#[tokio::test]
async fn an_ingestion_the_clock_disabled_is_deletable_without_a_stop_call() {
    // (case, app, tenant, destinations, identifiers by ARN)
    let cases: Vec<(&str, &str, &str, usize, bool)> = vec![
        (
            "one destination, by ARN",
            "CISCO",
            "tenant-cisco",
            1,
            true,
        ),
        (
            "two destinations, by bare uuid",
            "DROPBOX",
            "tenant-dropbox",
            2,
            false,
        ),
    ];

    for (case, app, tenant, created, by_arn) in cases {
        let emulator = Emulator::new();
        let (bundle, ingestion) = bundle_with_ingestion(&emulator, app, tenant, case).await;
        let targets = destinations(&emulator, &bundle, &ingestion, created, case).await;
        let started = start_ingestion(&emulator, &bundle, &ingestion).await;
        assert_eq!(
            started.status, 200,
            "case {case:?}: expected the ingestion to start, got {} with body {}",
            started.status, started.body
        );
        for target in &targets {
            let removed = delete_destination(
                &emulator,
                &bundle,
                &ingestion,
                &as_identifier(target, by_arn),
            )
            .await;
            assert_eq!(
                removed.status, 204,
                "case {case:?}: expected the destination to be deleted, got {} with body {}",
                removed.status, removed.body
            );
        }

        let refused = emulator
            .delete_ingestion(&bundle, &as_identifier(&ingestion, by_arn))
            .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400 while the ingestion is still enabled, got {} with body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {}",
            refused.body
        );
        let message = refused
            .body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            message.contains(INGESTION_STATE_ENABLED) && message.contains(&ingestion),
            "case {case:?}: the refusal must name the ingestion and the state it is in, got {message:?}"
        );

        tokio::time::sleep(PAST_THE_DISABLE_SPAN).await;

        let deleted = emulator
            .delete_ingestion(&bundle, &as_identifier(&ingestion, by_arn))
            .await;
        assert_eq!(
            deleted.status, 204,
            "case {case:?}: expected HTTP 204 once the disable is due, got {} with body {}",
            deleted.status, deleted.body
        );
        let read = emulator.get_ingestion(&bundle, &ingestion).await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: expected the deleted ingestion to be gone, got {} with body {}",
            read.status, read.body
        );
    }
}

/// RULE (invariant): an owed disable is a state, not a queued call — `StopIngestion`
/// has nothing left to stop once it is due (the modelled `ConflictException`,
/// HTTP 409), while `StartIngestion` is the move back to `enabled` and discards
/// the owed disable, so the ingestion stays `enabled` however much more of the
/// span elapses.
///
/// The table spans whether the start happens BEFORE the owed disable falls due or
/// after it: either way the ingestion a start left behind is `enabled`.
#[tokio::test]
async fn an_owed_disable_is_discarded_by_an_explicit_state_call() {
    // (case, app, tenant, wait for the disable before starting again)
    let cases: Vec<(&str, &str, &str, bool)> = vec![
        (
            "started again before the disable is due",
            "SMARTSHEET",
            "tenant-smartsheet",
            false,
        ),
        (
            "started again after the disable is due",
            "GOOGLE",
            "tenant-google",
            true,
        ),
    ];

    for (case, app, tenant, wait_first) in cases {
        let emulator = Emulator::new();
        let (bundle, ingestion) = bundle_with_ingestion(&emulator, app, tenant, case).await;
        let targets = destinations(&emulator, &bundle, &ingestion, 1, case).await;
        let started = start_ingestion(&emulator, &bundle, &ingestion).await;
        assert_eq!(
            started.status, 200,
            "case {case:?}: expected the ingestion to start, got {} with body {}",
            started.status, started.body
        );
        let removed = delete_destination(&emulator, &bundle, &ingestion, &targets[0]).await;
        assert_eq!(
            removed.status, 204,
            "case {case:?}: expected the destination to be deleted, got {} with body {}",
            removed.status, removed.body
        );

        if wait_first {
            tokio::time::sleep(PAST_THE_DISABLE_SPAN).await;
            // Nothing is left to stop: the clock already disabled it.
            let stopped = stop_ingestion(&emulator, &bundle, &ingestion).await;
            assert_eq!(
                stopped.status, 409,
                "case {case:?}: expected HTTP 409 from a stop of a disabled ingestion, got {} with body {}",
                stopped.status, stopped.body
            );
            assert_eq!(
                stopped.error_code(),
                "ConflictException",
                "case {case:?}: expected ConflictException, got {}",
                stopped.body
            );
        }

        let restarted = start_ingestion(&emulator, &bundle, &ingestion).await;
        let expected_status = if wait_first { 200 } else { 409 };
        assert_eq!(
            restarted.status, expected_status,
            "case {case:?}: expected HTTP {expected_status} from the second start, got {} with body {}",
            restarted.status, restarted.body
        );
        if !wait_first {
            // The ingestion is still `enabled` here, so there is nothing to
            // start; stopping and starting it is the way back, and the start is
            // what discards the disable the clock owed.
            let stopped = stop_ingestion(&emulator, &bundle, &ingestion).await;
            assert_eq!(
                stopped.status, 200,
                "case {case:?}: expected the stop to succeed, got {} with body {}",
                stopped.status, stopped.body
            );
            let again = start_ingestion(&emulator, &bundle, &ingestion).await;
            assert_eq!(
                again.status, 200,
                "case {case:?}: expected the start to succeed, got {} with body {}",
                again.status, again.body
            );
        }

        // However much of the span elapses, an explicit start is the last word on
        // the state: the disable the destination loss owed is gone.
        tokio::time::sleep(PAST_THE_DISABLE_SPAN).await;
        let state = ingestion_state(&emulator, &bundle, &ingestion).await;
        assert_eq!(
            state, INGESTION_STATE_ENABLED,
            "case {case:?}: expected the started ingestion to stay {INGESTION_STATE_ENABLED:?}, got {state:?}"
        );
        let listed = listed_state(&emulator, &bundle, &ingestion).await;
        assert_eq!(
            listed, INGESTION_STATE_ENABLED,
            "case {case:?}: expected ListIngestions to agree, got {listed:?}"
        );
    }
}
