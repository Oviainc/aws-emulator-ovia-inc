// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `ListConfigurationCheckDefinitions`, driven over the wire.
//!
//! Every case goes through the real router (the harness `tests/all/application_lifecycle.rs`
//! owns), so the restJson1 routing, the boundary constraint validation, the handler and
//! the response serialization all run exactly as they do in the emulator binary. Nothing
//! reaches into `crate::state` directly: the catalogue the listing reports is only
//! believed against what `StartConfigurationChecks` actually starts.
//!
//! RULE under test: the listing reports the catalogue of configuration checks the SERVICE
//! supports — one definition per value of the model's `ConfigurationCheckType` enum, each
//! carrying that value as its `Id` and the application types a start of that check is
//! accepted for — handed out in pages of at most `MaxResults` whose `NextToken` resumes
//! exactly where the previous page stopped; the catalogue does not depend on what an
//! account has registered or run, is the same at every regional endpoint, and the call
//! changes nothing a later caller sees.
//!
//! The rows span what the rule generalises over: the model's whole
//! `ConfigurationCheckType` catalogue (not one check), every `ApplicationType` a
//! definition reports, page sizes from one definition to past the whole catalogue
//! including each published `MaxResults` bound and one past it, the absent `MaxResults`,
//! several regional endpoints, and stores holding nothing as well as a store holding a
//! registered application with runs against it.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_ssmsap_ssmsap::ops::types::{
    APPLICATION_TYPE_VALUES, CONFIGURATION_CHECK_TYPE_VALUES,
};
use service_com_amazonaws_ssmsap_ssmsap::state::MAX_PAGE_SIZE;

use crate::application_lifecycle::{valid_registration, Emulator, Reply};
use crate::configuration_checks::{field, started, string_field};
use crate::validation_message_rendering::{message, VALIDATION_EXCEPTION};

/// The regional endpoints the catalogue is read at. The catalogue is the service's own,
/// so it cannot differ between them; more than one row is what shows that.
const REGIONS: &[&str] = &["us-east-1", "eu-west-1", "ap-southeast-2"];

impl Emulator {
    async fn list_definitions(&self, body: Value) -> Reply {
        self.send("/list-configuration-check-definitions", body)
            .await
    }
}

/// The `ConfigurationChecks` of a listing reply.
fn listed(reply: &Reply) -> Vec<Value> {
    reply
        .member("ConfigurationChecks")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The `ApplicableApplicationTypes` one definition reports.
///
/// The member is read through `crate::configuration_checks::field`, the reading of an
/// optional member every suite of this target shares.
fn applicable_types(definition: &Value) -> Vec<String> {
    field(definition, "ApplicableApplicationTypes")
        .and_then(Value::as_array)
        .map(|types| {
            types
                .iter()
                .map(|value| value.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Follow the listing's pages from the first to the last, returning the definitions in
/// the order the pages reported them. Asserts the paging contract as it walks: no page
/// holds more than `limit` definitions, and only the last page comes without a
/// `NextToken`.
async fn walk_pages(emulator: &Emulator, mut body: Value, case: &str, limit: usize) -> Vec<Value> {
    let mut rows: Vec<Value> = Vec::new();
    let mut pages = 0;
    loop {
        let reply = emulator.list_definitions(body.clone()).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?} page {pages}: expected 200, got {reply:?}"
        );
        let page = listed(&reply);
        assert!(
            page.len() <= limit,
            "case {case:?} page {pages}: a page holds at most {limit} definitions, got {}",
            page.len()
        );
        rows.extend(page);
        pages += 1;
        // A listing that kept handing out tokens would never terminate; the walk is
        // bounded by one page per definition plus the final tokenless page.
        assert!(
            pages <= rows.len() + 1,
            "case {case:?}: the paging must terminate, got {pages} pages for {} definitions",
            rows.len()
        );
        match reply.member("NextToken").and_then(Value::as_str) {
            Some(token) => {
                let object = body.as_object_mut().expect("the body is a JSON object");
                object.insert("NextToken".to_string(), json!(token));
            }
            None => return rows,
        }
    }
}

/// Every check in the model's catalogue, in the order the listing reports them (by `Id`).
fn modelled_catalogue() -> Vec<String> {
    let mut checks: Vec<String> = CONFIGURATION_CHECK_TYPE_VALUES
        .iter()
        .map(|check| (*check).to_string())
        .collect();
    checks.sort();
    checks
}

/// What every definition must carry, whichever page it came on.
fn assert_well_formed(case: &str, definition: &Value) {
    let id = string_field(definition, "Id");
    assert!(
        CONFIGURATION_CHECK_TYPE_VALUES.contains(&id.as_str()),
        "case {case:?}: a definition's Id must be a modelled ConfigurationCheckType, got \
         {definition:?}"
    );
    let types = applicable_types(definition);
    assert!(
        !types.is_empty(),
        "case {case:?}: a definition reports the application types it applies to, got \
         {definition:?}"
    );
    for application_type in &types {
        assert!(
            APPLICATION_TYPE_VALUES.contains(&application_type.as_str()),
            "case {case:?}: ApplicableApplicationTypes must hold modelled ApplicationType \
             values, got {application_type:?} in {definition:?}"
        );
    }
}

/// RULE: the listing reports one definition per modelled `ConfigurationCheckType`, with
/// that value as its `Id` — whatever the account has registered or run, and at whichever
/// regional endpoint it is asked.
#[tokio::test]
async fn the_listing_reports_one_definition_per_modelled_check() {
    let emulator = Emulator::new();
    let expected = modelled_catalogue();

    // A store holding nothing at all: the catalogue is the service's, so it does not
    // wait on a registration.
    for region in REGIONS {
        let reply = emulator
            .send_in_region(region, "/list-configuration-check-definitions", json!({}))
            .await;
        assert_eq!(
            reply.status, 200,
            "case {region:?}: expected 200 from ListConfigurationCheckDefinitions, got {reply:?}"
        );
        let rows = listed(&reply);
        let ids: Vec<String> = rows.iter().map(|row| string_field(row, "Id")).collect();
        assert_eq!(
            ids, expected,
            "case {region:?}: expected the whole modelled catalogue in Id order, got {ids:?}"
        );
        assert!(
            reply.member("NextToken").is_none(),
            "case {region:?}: the whole catalogue fits one page, so no token is handed out, got \
             {reply:?}"
        );
        for row in &rows {
            assert_well_formed(region, row);
        }
    }

    // A store holding a registered application with every check started against it
    // reports the SAME catalogue: what exists to run does not depend on what has run.
    let application_id = "HDB-DEFINITIONS";
    emulator.with_application(application_id).await;
    let start = emulator
        .start_checks(json!({"ApplicationId": application_id}))
        .await;
    assert_eq!(
        start.status, 200,
        "expected 200 from StartConfigurationChecks, got {start:?}"
    );
    let after = emulator.list_definitions(json!({})).await;
    let ids: Vec<String> = listed(&after)
        .iter()
        .map(|row| string_field(row, "Id"))
        .collect();
    assert_eq!(
        ids, expected,
        "case \"after a run\": the catalogue must not depend on what an account has run, got \
         {ids:?}"
    );
}

/// RULE: a caller that follows the tokens sees every definition exactly once, in `Id`
/// order, whatever page size it asked for; a page size outside the model's declared range
/// and a token this service did not issue are refused.
#[tokio::test]
async fn following_the_tokens_reports_every_definition_exactly_once() {
    let emulator = Emulator::new();
    let expected = modelled_catalogue();

    // Page sizes spanning one definition per page, a partial page, exactly the whole
    // catalogue, past it, and each published `MaxResults` bound (1 and 50).
    let sizes: Vec<i32> = vec![
        1,
        2,
        expected.len() as i32,
        expected.len() as i32 + 1,
        MAX_PAGE_SIZE,
    ];
    for max_results in sizes {
        let case = format!("MaxResults {max_results}");
        let rows = walk_pages(
            &emulator,
            json!({"MaxResults": max_results}),
            &case,
            max_results as usize,
        )
        .await;
        let ids: Vec<String> = rows.iter().map(|row| string_field(row, "Id")).collect();
        assert_eq!(
            ids, expected,
            "case {case:?}: the pages together must report the whole catalogue once, in Id \
             order, got {ids:?}"
        );
        for row in &rows {
            assert_well_formed(&case, row);
        }
    }

    // The absent member takes the largest page the range allows, so one call reports the
    // whole catalogue.
    let rows = walk_pages(
        &emulator,
        json!({}),
        "MaxResults absent",
        MAX_PAGE_SIZE as usize,
    )
    .await;
    let ids: Vec<String> = rows.iter().map(|row| string_field(row, "Id")).collect();
    assert_eq!(
        ids, expected,
        "case \"MaxResults absent\": expected the whole catalogue, got {ids:?}"
    );

    // A page size outside the model's declared range is refused at the boundary, on
    // either side of it.
    for max_results in [0, MAX_PAGE_SIZE + 1] {
        let reply = emulator
            .list_definitions(json!({"MaxResults": max_results}))
            .await;
        assert_eq!(
            reply.status, 400,
            "case {max_results}: expected 400, got {}: {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.code(),
            VALIDATION_EXCEPTION,
            "case {max_results}: expected {VALIDATION_EXCEPTION}, got {reply:?}"
        );
    }

    // A token this service did not issue is refused rather than resumed from. Every row
    // satisfies the model's `NextToken` pattern (16..2048 characters), so the refusal is
    // the handler's and not the boundary validator's.
    for token in [
        "0000000000000000",
        "not-a-token-this-service-issued",
        "ffffffffffffffffffffffffffffffff",
    ] {
        let reply = emulator.list_definitions(json!({"NextToken": token})).await;
        assert_eq!(
            reply.status, 400,
            "case {token:?}: expected 400, got {}: {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.code(),
            VALIDATION_EXCEPTION,
            "case {token:?}: expected {VALIDATION_EXCEPTION}, got {reply:?}"
        );
        assert!(
            message(&reply).contains(token),
            "case {token:?}: the refusal must name the token it could not resume from, got {:?}",
            message(&reply)
        );
    }
}

/// RULE: a definition's `ApplicableApplicationTypes` are the application types a start of
/// that check is accepted for — so the catalogue and `StartConfigurationChecks` cannot
/// disagree about what this service supports.
#[tokio::test]
async fn a_definition_names_the_application_types_its_check_can_be_started_against() {
    let emulator = Emulator::new();
    let definitions = listed(&emulator.list_definitions(json!({})).await);
    assert!(
        !definitions.is_empty(),
        "expected the listing to report a catalogue to check against, got {definitions:?}"
    );

    for definition in &definitions {
        let check = string_field(definition, "Id");
        for application_type in applicable_types(definition) {
            let case = format!("{check} on {application_type}");
            // One application per row, named after the row: a registration is per
            // application id, and the row asserts the start is accepted for that TYPE.
            let application_id = format!("APP-{check}-{application_type}");
            let registration = emulator
                .send(
                    "/register-application",
                    valid_registration(&application_id, &application_type),
                )
                .await;
            assert_eq!(
                registration.status, 200,
                "case {case:?}: expected 200 from RegisterApplication, got {registration:?}"
            );

            let reply = emulator
                .start_checks(json!({
                    "ApplicationId": application_id,
                    "ConfigurationCheckIds": [check],
                }))
                .await;
            assert_eq!(
                reply.status, 200,
                "case {case:?}: a catalogued check must start against an application type the \
                 definition names, got {reply:?}"
            );
            let operations = started(&reply);
            let started_checks: BTreeSet<String> = operations
                .iter()
                .map(|operation| string_field(operation, "ConfigurationCheckId"))
                .collect();
            assert_eq!(
                started_checks,
                BTreeSet::from([check.clone()]),
                "case {case:?}: expected the started run to be the check the definition names, \
                 got {operations:?}"
            );
        }
    }
}

/// RULE (a report changes nothing): reading the catalogue leaves the service exactly as
/// the next caller would have found it — repeated listings report an identical body, and
/// no configuration check operation appears because the catalogue was read.
#[tokio::test]
async fn reading_the_catalogue_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();
    let application_id = "HDB-DEFINITIONS-READ";
    emulator.with_application(application_id).await;

    let first = emulator.list_definitions(json!({})).await;
    assert_eq!(first.status, 200, "expected 200, got {first:?}");

    for attempt in 1..4 {
        let later = emulator.list_definitions(json!({})).await;
        assert_eq!(
            later.body, first.body,
            "read {attempt}: every read of the catalogue must report the same body, got {later:?}"
        );
    }

    // The listing is not a run: nothing was started against the application by reading
    // which checks exist.
    let operations = emulator
        .send(
            "/list-configuration-check-operations",
            json!({"ApplicationId": application_id, "ListMode": "ALL_OPERATIONS"}),
        )
        .await;
    assert_eq!(
        operations.status, 200,
        "expected 200 from ListConfigurationCheckOperations, got {operations:?}"
    );
    assert!(
        operations
            .member("ConfigurationCheckOperations")
            .and_then(Value::as_array)
            .is_none_or(|rows| rows.is_empty()),
        "reading the catalogue must start nothing, got {operations:?}"
    );
}
