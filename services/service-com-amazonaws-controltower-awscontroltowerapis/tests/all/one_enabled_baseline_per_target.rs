// SPDX-License-Identifier: CC0-1.0

//! How `EnableBaseline` REFUSES a repeat of a pair it already holds, end to end over HTTP,
//! with `GetEnabledBaseline`, `ListEnabledBaselines` and `GetBaselineOperation` counting
//! what each refusal left behind.
//!
//! Driven through the same [`MultiServiceRouter`](smithy_server_runtime::MultiServiceRouter)
//! the production emulator dispatches with, so what these tests exercise is the wire
//! behaviour a client sees — status codes, error codes and JSON bodies — not the handlers'
//! Rust signatures.
//!
//! The rule under test, asserted over a TABLE of inputs rather than one case: a request to
//! enable a resource on a target that already holds one is classified by the STATE of that
//! enablement — while it is still being applied the repeat would race a change in flight,
//! the inconsistent state `ConflictException` (409) reports, and once it is applied nothing
//! is in flight and the request merely asks for a resource that already exists, which
//! `ValidationException` (400) reports; either way the refusal names the enablement it
//! collided with and writes nothing. It is the rule `EnableControl` already follows, and
//! `CreateLandingZone` one level up; the control side of it is asserted by
//! `tests/all/one_enabled_control_per_target.rs`, whose table shape these cases mirror
//! rather than restate.
//!
//! The table's rows are every ORDERED pair of two distinct identifiers of one baseline, so
//! each row enables under one form and repeats under another, at a different version — the
//! two things the pair rule generalises over besides the state of the existing enablement.
//! The forms come from `catalog_identifier_equivalence::baseline_identifier_forms`, which
//! names its source (`smithy_query(query="shape", shape="BaselineArn")` on this crate's
//! `smithy-model/`), and the baselines from
//! `service_..._awscontroltowerapis::state::BASELINE_CATALOG` — the catalog
//! `EnableBaseline` resolves against — so the rows cover entries beyond the one an issue
//! happened to cite. The status and operation-type values written out here come from
//! `smithy_query(query="shape", shape="EnablementStatus,BaselineOperationType")`.

use service_com_amazonaws_controltower_awscontroltowerapis::state::{
    CatalogBaseline, BASELINE_CATALOG,
};

use crate::baseline_operations::{get_operation, member};
use crate::catalog_identifier_equivalence::{
    assert_refused, baseline_identifier_forms, enable_baseline, listed_arns, oldest_and_newest, ou,
};
use crate::enable_baseline_validation::enable;
use crate::in_flight_configuration_change_drift::{baseline_details, baseline_status};
use crate::landing_zone_lifecycle::{call, past_the_settle_span, set_up_emulator, Answer};

/// One row: a baseline enabled under one of its identifiers at one version, and repeated
/// under another identifier at another version, on a target of its own.
struct Row {
    /// What the row is, for the assertion messages.
    case: String,
    /// The identifier the accepted enablement named the baseline by.
    enabled_with: String,
    /// The version that accepted call asked for.
    enabled_version: &'static str,
    /// Another identifier of the SAME baseline, which the repeat names it by.
    repeated_with: String,
    /// The version the repeat asks for, which is not part of what identifies the pair.
    repeated_version: &'static str,
    /// The organizational unit every call of the row applies the baseline to.
    target: String,
}

/// The table: every ordered pair of two DISTINCT identifier forms of one baseline, with the
/// baseline rotating through the catalog so no row's outcome can depend on one catalog
/// entry, and each row on its own target so the rows do not interact.
fn rows() -> Vec<Row> {
    let first = BASELINE_CATALOG.first().expect("a catalog baseline");
    let form_count = baseline_identifier_forms(first.id).len();
    assert!(
        BASELINE_CATALOG.len() > 1 && form_count > 1,
        "the table needs more than one catalog baseline and more than one identifier \
         form, got {} baselines and {form_count} forms",
        BASELINE_CATALOG.len()
    );

    let mut rows: Vec<Row> = Vec::new();
    for enabled_index in 0..form_count {
        for repeated_index in 0..form_count {
            if enabled_index == repeated_index {
                continue;
            }
            let baseline: &CatalogBaseline =
                &BASELINE_CATALOG[rows.len() % BASELINE_CATALOG.len()];
            let (oldest, newest) = oldest_and_newest(baseline);
            let forms = baseline_identifier_forms(baseline.id);
            let (enabled_case, enabled_with) = forms[enabled_index].clone();
            let (repeated_case, repeated_with) = forms[repeated_index].clone();
            rows.push(Row {
                case: format!(
                    "{} enabled by {enabled_case} at {oldest}, repeated with \
                     {repeated_case} at {newest}",
                    baseline.name
                ),
                enabled_with,
                enabled_version: oldest,
                repeated_with,
                repeated_version: newest,
                target: ou(&format!("b{enabled_index}x{repeated_index}")),
            });
        }
    }
    rows
}

/// Assert a refused repeat: the status, error code and wrote-nothing rule
/// [`assert_refused`] states, and — what this suite adds — a message naming the enablement
/// that already holds the pair, so the caller is told which resource the request collided
/// with rather than only that it did.
fn assert_refused_naming(
    case: &str,
    refused: &Answer,
    status: u16,
    code: &str,
    existing_arn: &str,
) {
    assert_refused(case, refused, status, code);
    let message = refused.string("message");
    assert!(
        message.contains(existing_arn),
        "case {case:?}: the refusal must name the enablement already holding the pair \
         ({existing_arn}), got {message:?}"
    );
}

/// `statusSummary.lastOperationIdentifier` of an `EnabledBaselineDetails`.
fn last_operation(details: &serde_json::Value) -> Option<&str> {
    details
        .get("statusSummary")
        .and_then(|summary| summary.get("lastOperationIdentifier"))
        .and_then(serde_json::Value::as_str)
}

/// RULE: a repeat of a pair already enabled is 409 `ConflictException` while that
/// enablement is still being applied and 400 `ValidationException` once it is applied, and
/// neither refusal mints an enablement, an operation or a slot of the enablement cap.
#[tokio::test]
async fn a_repeated_enablement_is_classified_by_the_state_of_the_one_it_collided_with() {
    let router = set_up_emulator().await;
    let mut enabled: Vec<(Row, String, String)> = Vec::new();

    for row in rows() {
        let accepted = enable_baseline(
            &router,
            &row.enabled_with,
            row.enabled_version,
            &row.target,
        )
        .await;
        assert_eq!(
            accepted.status, 200,
            "case {:?}: expected 200, got {} body {:?}",
            row.case, accepted.status, accepted.body
        );
        let arn = accepted.string("arn");
        let operation_identifier = accepted.string("operationIdentifier");

        // The in-flight repeat is asserted on the enablement this iteration just started,
        // rather than after the whole table, because the settle span is short on purpose:
        // a case that enabled every row first would race the clock out of the in-flight
        // state.
        let while_deploying = enable_baseline(
            &router,
            &row.repeated_with,
            row.repeated_version,
            &row.target,
        )
        .await;
        assert_refused_naming(
            &format!("{} while the first enablement is being applied", row.case),
            &while_deploying,
            409,
            "ConflictException",
            &arn,
        );

        enabled.push((row, arn, operation_identifier));
    }

    tokio::time::sleep(past_the_settle_span()).await;

    for (row, arn, operation_identifier) in &enabled {
        // Applied, the enablement reports a terminal status: nothing is in flight for the
        // repeat below to race.
        let details = baseline_details(&router, &row.case, arn).await;
        assert_eq!(
            (baseline_status(&details), last_operation(&details)),
            (Some("SUCCEEDED"), Some(operation_identifier.as_str())),
            "case {:?}: expected the deployed enablement of the pair, got {details:?}",
            row.case
        );

        let deployed = enable_baseline(
            &router,
            &row.repeated_with,
            row.repeated_version,
            &row.target,
        )
        .await;
        assert_refused_naming(
            &format!("{} once the first enablement is applied", row.case),
            &deployed,
            400,
            "ValidationException",
            arn,
        );

        // ONE enabled baseline on the target, whatever the two calls named the baseline by
        // and whatever version each asked for.
        let listed = call(
            &router,
            "/list-enabled-baselines",
            serde_json::json!({
                "filter": { "targetIdentifiers": [row.target] },
                "maxResults": 100,
            }),
        )
        .await;
        assert_eq!(
            listed_arns(&listed, "enabledBaselines"),
            vec![arn.clone()],
            "case {:?}: expected the one enablement [{arn:?}] on the target, got {:?}",
            row.case,
            listed.body
        );

        // ONE lifecycle: the refusals started no operation of their own, so the accepted
        // call's record is still the operation the enablement carries, and it is the one
        // that call minted.
        let (answer, operation) = get_operation(&router, operation_identifier).await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: get-baseline-operation expected 200, got {} body {:?}",
            row.case, answer.status, answer.body
        );
        assert_eq!(
            (
                member(&operation, "operationType"),
                member(&operation, "status"),
            ),
            (Some("ENABLE_BASELINE"), Some("SUCCEEDED")),
            "case {:?}: expected the accepted enablement's own record, got {operation:?}",
            row.case
        );
        let after = baseline_details(&router, &row.case, arn).await;
        assert_eq!(
            last_operation(&after),
            Some(operation_identifier.as_str()),
            "case {:?}: a refused repeat must not start an operation, got {after:?}",
            row.case
        );
    }

    // The store holds exactly the accepted enablements: no refusal took a slot of the
    // enablement cap this operation reports as its quota.
    let everything = call(
        &router,
        "/list-enabled-baselines",
        serde_json::json!({ "maxResults": 100 }),
    )
    .await;
    let mut expected: Vec<String> = enabled.iter().map(|(_, arn, _)| arn.clone()).collect();
    expected.sort();
    assert_eq!(
        listed_arns(&everything, "enabledBaselines"),
        expected,
        "expected one enablement per row and nothing else, got {:?}",
        everything.body
    );
}

/// RULE (the same classification, whatever the accepted request looked like): what decides
/// between the two refusals is whether a change is still in flight — not the spelling of
/// the repeat, the version it asks for, or whether the accepted call carried parameters.
#[tokio::test]
async fn the_classification_turns_only_on_whether_a_change_is_in_flight() {
    let router = set_up_emulator().await;
    let baseline = BASELINE_CATALOG.first().expect("a catalog baseline");
    let (version, newest) = oldest_and_newest(baseline);
    let forms = baseline_identifier_forms(baseline.id);
    let target = ou("settled");

    // An enablement carrying a parameter list, enabled through the OTHER helper this
    // crate's suites drive `EnableBaseline` with, so the classification does not depend on
    // the shape of the accepted request either.
    let parameters = (!baseline.parameters.is_empty()).then(|| {
        serde_json::json!([{ "key": baseline.parameters[0], "value": true }])
    });
    let accepted = enable(
        &router,
        &forms[0].1,
        version,
        &target,
        parameters.clone(),
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "the accepted enablement: expected 200, got {} body {:?}",
        accepted.status, accepted.body
    );
    let arn = accepted.string("arn");

    // In flight, both spellings of the repeat race the change the accepted call started.
    for (case, identifier, asked) in [
        ("the same identifier at the same version", &forms[0].1, version),
        ("another identifier at another version", &forms[3].1, newest),
    ] {
        let again = enable(&router, identifier, asked, &target, parameters.clone()).await;
        assert_refused_naming(
            &format!("in flight, {case}"),
            &again,
            409,
            "ConflictException",
            &arn,
        );
    }

    tokio::time::sleep(past_the_settle_span()).await;

    // Applied, every spelling of the repeat asks for a resource that already exists.
    for (case, identifier, asked) in [
        ("the same identifier at the same version", &forms[0].1, version),
        ("another identifier at another version", &forms[3].1, newest),
    ] {
        let again = enable(&router, identifier, asked, &target, parameters.clone()).await;
        assert_refused_naming(
            &format!("applied, {case}"),
            &again,
            400,
            "ValidationException",
            &arn,
        );
    }

    // The enablement the refusals collided with is untouched: still the one record, still
    // deployed, still carrying the version the accepted call asked for.
    let details = baseline_details(&router, "the collided-with enablement", &arn).await;
    assert_eq!(
        (
            baseline_status(&details),
            details
                .get("baselineVersion")
                .and_then(serde_json::Value::as_str),
        ),
        (Some("SUCCEEDED"), Some(version)),
        "a refused repeat must change nothing about the enablement, got {details:?}"
    );
}
