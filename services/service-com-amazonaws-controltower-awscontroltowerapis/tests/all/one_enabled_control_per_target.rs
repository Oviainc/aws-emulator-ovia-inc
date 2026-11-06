// SPDX-License-Identifier: CC0-1.0

//! The CARDINALITY of a control enablement, end to end over HTTP: `EnableControl` with
//! `ListEnabledControls`, `ListControlOperations` and `GetEnabledControl` counting what it
//! left behind.
//!
//! Driven through the same `MultiServiceRouter` the production emulator dispatches with,
//! so what these tests exercise is the wire behaviour a client sees — status codes, error
//! codes and JSON bodies — not the handlers' Rust signatures.
//!
//! The rule under test, asserted over a TABLE of inputs rather than one case: a control is
//! identified by the catalog entry its identifier resolves to, so one control on one
//! target is ONE enabled control with ONE operation lifecycle, whichever of the control's
//! published identifier forms each call names it by — a repeat refused while the first
//! enablement is deploying (409 `ConflictException`) or once it is deployed (400
//! `ValidationException`) names the enablement it collided with and adds neither a second
//! enabled control, nor a second operation record, nor a second slot of the store's
//! enablement cap.
//!
//! What this suite asserts is that CARDINALITY, and the refusal's wording; that every
//! identifier form of a control is accepted, refused and SELECTED by a listing's filter
//! alike is the subject of `tests/all/catalog_identifier_equivalence.rs`, whose helpers
//! these cases reuse rather than restate.
//!
//! The table's rows are every ORDERED pair of two distinct identifiers of one control, so
//! each row enables under one form and repeats under another; the forms themselves come
//! from `catalog_identifier_equivalence::control_identifier_forms`, which names its source
//! (`smithy_query(query="shape", shape="ControlIdentifier")` on this crate's
//! `smithy-model/`, and the two published control ARN forms), and the controls come from
//! `service_..._awscontroltowerapis::state::CONTROL_CATALOG` — the catalog `EnableControl`
//! resolves against — so the rows cover entries beyond the one an issue happened to cite.
//! The operation type and status values written out here come from
//! `smithy_query(query="shape", shape="ControlOperationType,EnablementStatus")`.

use service_com_amazonaws_controltower_awscontroltowerapis::state::{
    CatalogControl, CONTROL_CATALOG,
};
use crate::catalog_identifier_equivalence::{
    assert_refused, control_identifier_forms, enable_control, listed_arns, ou,
};
use crate::enabled_control_lifecycle::{
    details, entries, last_operation, member, status_summary_status,
};
use crate::landing_zone_lifecycle::{call, past_the_settle_span, set_up_emulator, Answer};

/// One row: a control enabled under one of its identifiers and repeated under another, on
/// a target of its own.
struct Row {
    /// What the row is, for the assertion messages.
    case: String,
    /// The identifier the accepted enablement named the control by.
    enabled_with: String,
    /// Another identifier of the SAME control, which the repeat names it by.
    repeated_with: String,
    /// The organizational unit both calls apply it to.
    target: String,
}

/// The table: every ordered pair of two DISTINCT identifier forms of one control, with the
/// control rotating through the catalog's enablable entries so no row's outcome can depend
/// on one catalog entry, and each row on its own target so the rows do not interact.
fn rows() -> Vec<Row> {
    let enablable: Vec<&'static CatalogControl> = CONTROL_CATALOG
        .iter()
        .filter(|control| control.enablable)
        .collect();
    let first = *enablable
        .first()
        .expect("the catalog publishes an enablable control");
    let form_count = control_identifier_forms(first).len();
    assert!(
        enablable.len() > 1 && form_count > 1,
        "the table needs more than one enablable control and more than one identifier \
         form, got {} controls and {form_count} forms",
        enablable.len()
    );

    let mut rows: Vec<Row> = Vec::new();
    for enabled_index in 0..form_count {
        for repeated_index in 0..form_count {
            if enabled_index == repeated_index {
                continue;
            }
            let control = enablable[rows.len() % enablable.len()];
            let forms = control_identifier_forms(control);
            let (enabled_case, enabled_with) = forms[enabled_index].clone();
            let (repeated_case, repeated_with) = forms[repeated_index].clone();
            rows.push(Row {
                case: format!(
                    "{} enabled by {enabled_case}, repeated with {repeated_case}",
                    control.name
                ),
                enabled_with,
                repeated_with,
                target: ou(&format!("r{enabled_index}x{repeated_index}")),
            });
        }
    }
    rows
}

/// Assert a refused repeat: the status, error code and minted-nothing rule the sibling
/// suite's [`assert_refused`] states, and — what this suite adds — a message naming the
/// enablement that already holds the pair, so the caller is told which resource the
/// request collided with rather than only that it did.
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

/// RULE: one control on one target is one enabled control with one operation lifecycle,
/// whichever of the control's published identifiers each call names it by.
#[tokio::test]
async fn one_control_on_one_target_is_one_enablement_and_one_lifecycle() {
    let router = set_up_emulator().await;
    let mut enabled: Vec<(Row, String, String)> = Vec::new();

    for row in rows() {
        let accepted = enable_control(&router, &row.enabled_with, &row.target).await;
        assert_eq!(
            accepted.status, 200,
            "case {:?}: expected 200, got {} body {:?}",
            row.case, accepted.status, accepted.body
        );
        let arn = accepted.string("arn");
        let operation_identifier = accepted.string("operationIdentifier");

        // The repeat is asserted on the enablement this iteration just started, rather
        // than after the whole table, because the settle span is short on purpose: a case
        // that enabled every row first would race the clock out of the in-flight state.
        let while_deploying = enable_control(&router, &row.repeated_with, &row.target).await;
        assert_refused_naming(
            &format!("{} while the first enablement is deploying", row.case),
            &while_deploying,
            409,
            "ConflictException",
            &arn,
        );

        enabled.push((row, arn, operation_identifier));
    }

    tokio::time::sleep(past_the_settle_span()).await;

    for (row, arn, operation_identifier) in &enabled {
        // Deployed, the repeat asks for something that already exists.
        let deployed = enable_control(&router, &row.repeated_with, &row.target).await;
        assert_refused_naming(
            &format!("{} once the first enablement is deployed", row.case),
            &deployed,
            400,
            "ValidationException",
            arn,
        );

        // ONE enabled control on the target, whatever the two calls named the control by.
        let listed = call(
            &router,
            "/list-enabled-controls",
            serde_json::json!({ "targetIdentifier": row.target }),
        )
        .await;
        assert_eq!(
            listed_arns(&listed, "enabledControls"),
            vec![arn.clone()],
            "case {:?}: expected the one enablement [{arn:?}] on the target, got {:?}",
            row.case,
            listed.body
        );

        // ONE lifecycle: the accepted call's record is the only one the log holds for the
        // pair, and the refusals appended nothing.
        let operations = call(
            &router,
            "/list-control-operations",
            serde_json::json!({
                "filter": {
                    "controlIdentifiers": [row.repeated_with],
                    "targetIdentifiers": [row.target],
                },
            }),
        )
        .await;
        let records = entries(operations.get("controlOperations"));
        assert_eq!(
            records.len(),
            1,
            "case {:?}: expected one operation record for the pair, got {:?}",
            row.case,
            operations.body
        );
        let record = &records[0];
        assert_eq!(
            (
                member(record, "operationIdentifier"),
                member(record, "operationType"),
                member(record, "enabledControlIdentifier"),
                member(record, "status"),
            ),
            (
                Some(operation_identifier.as_str()),
                Some("ENABLE_CONTROL"),
                Some(arn.as_str()),
                Some("SUCCEEDED"),
            ),
            "case {:?}: expected the accepted enablement's own record, got {record:?}",
            row.case
        );

        // The enablement reads back as the deployed one the refusals left alone.
        let read_back = details(&router, arn).await;
        assert_eq!(
            (
                status_summary_status(&read_back),
                last_operation(&read_back),
                member(&read_back, "targetIdentifier"),
            ),
            (
                Some("SUCCEEDED"),
                Some(operation_identifier.as_str()),
                Some(row.target.as_str()),
            ),
            "case {:?}: expected the deployed enablement of the pair, got {read_back:?}",
            row.case
        );
    }

    // The store holds exactly the accepted enablements: no refusal took a slot of the
    // enablement cap this operation reports as its quota.
    let everything = call(&router, "/list-enabled-controls", serde_json::json!({})).await;
    let mut expected: Vec<String> = enabled.iter().map(|(_, arn, _)| arn.clone()).collect();
    expected.sort();
    assert_eq!(
        listed_arns(&everything, "enabledControls"),
        expected,
        "expected one enablement per row and nothing else, got {:?}",
        everything.body
    );
}
