// SPDX-License-Identifier: CC0-1.0

//! Which refusal an update of an existing enablement answers with when the request is
//! itself unacceptable AND an operation is still changing the resource, end to end over
//! HTTP.
//!
//! Driven through the same [`MultiServiceRouter`](smithy_server_runtime::MultiServiceRouter)
//! the production emulator dispatches with, so what these tests exercise is the wire
//! behaviour a client sees — status codes, error codes and JSON bodies — not the handlers'
//! Rust signatures.
//!
//! The rule under test: a call that updates an existing enablement settles WHAT the request
//! asks for before it looks at the state of the resource, so a request naming a
//! configuration the resource's own catalog entry does not offer is refused with
//! `ValidationException` (400) in the same words whether or not an asynchronous operation is
//! still changing that resource, and the modelled `ConflictException` (409) answers only a
//! request the service would otherwise accept. The two statuses are the modelled ones, from
//! `smithy_query(query="operation", operation="UpdateEnabledBaseline")` and
//! `smithy_query(query="operation", operation="UpdateEnabledControl")`.
//!
//! The table is both catalogs themselves — `state::BASELINE_CATALOG` and the enablable
//! entries of `state::CONTROL_CATALOG`, the catalogs `ListBaselines`/`ListEnabledControls`
//! publish — so the rule is asserted for every resource kind this service updates and for
//! entries on both sides of what it generalises over: a baseline offering one version and
//! one offering several, an entry declaring a parameter and one declaring none. The rows
//! differ along the form of the unacceptable member (a version past the newest offered, a
//! version only another entry offers, a key no entry declares, a key only another entry
//! declares, a declared key given twice, and a version and a parameter both wrong), and the
//! state of the resource differs along which call put the operation in flight (the enable,
//! an update, a reset).
//!
//! Nothing here is written out against one recorded value: every version and parameter key a
//! row carries is derived from the catalog entry under test, and the message an in-flight
//! refusal carries is compared with the message the SAME request gets once the clock has made
//! everything due, rather than with a string this suite spells out.

use service_com_amazonaws_controltower_awscontroltowerapis::state::{
    CatalogBaseline, CatalogControl, BASELINE_CATALOG, CONTROL_CATALOG,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::baseline_catalog::served_arn;
use crate::enable_baseline_validation::{
    applied_parameters, assert_refused, enable, enabled_baseline_details, every_offered_version,
    member, ou, update,
};
use crate::enabled_control_lifecycle::{control, enable_with, ou as control_ou};
use crate::landing_zone_lifecycle::{call, past_the_settle_span, set_up_emulator, Answer};

/// A parameter key no catalog entry declares, so a `parameters` list carrying it configures
/// nothing whichever baseline or control it is sent to. Pinned as such by
/// [`no_catalog_entry_declares_the_undeclared_key`].
const UNDECLARED_KEY: &str = "NoSuchParameter";

/// The oldest version `baseline` offers — the catalog lists a baseline's versions oldest
/// first.
fn oldest_offered(baseline: &CatalogBaseline) -> &'static str {
    baseline
        .versions
        .first()
        .copied()
        .expect("a catalog baseline offers at least one version")
}

/// The newest version `baseline` offers.
fn newest_offered(baseline: &CatalogBaseline) -> &'static str {
    baseline
        .versions
        .last()
        .copied()
        .expect("a catalog baseline offers at least one version")
}

/// A version one major release past the newest `baseline` offers, so it is outside that
/// baseline's set however the catalog grows.
fn past_the_newest_offered(baseline: &CatalogBaseline) -> String {
    let major: u32 = newest_offered(baseline)
        .split('.')
        .next()
        .and_then(|component| component.parse().ok())
        .expect("a published version starts with a number");
    format!("{}.0", major + 1)
}

/// Every parameter key SOME catalog control declares, deduplicated — the pool the control
/// rows draw from, so a row can send one control a key only another one declares.
fn every_declared_control_parameter() -> Vec<&'static str> {
    let mut keys: Vec<&'static str> = CONTROL_CATALOG
        .iter()
        .flat_map(|control| control.parameters.iter().copied())
        .collect();
    keys.sort_unstable();
    keys.dedup();
    keys
}

/// One `UpdateEnabledBaseline` the service never accepts, whatever state the enablement is
/// in: `what` names the row, `version` is its `baselineVersion` and `parameters` its
/// `parameters` member (`None` omits it).
struct Unacceptable {
    what: String,
    version: String,
    parameters: Option<serde_json::Value>,
}

/// The unacceptable rows for `baseline`, derived from its catalog entry: a version it does
/// not offer, a `parameters` list it does not accept, or both.
fn unacceptable_rows(baseline: &CatalogBaseline) -> Vec<Unacceptable> {
    let newest = newest_offered(baseline).to_string();
    let past_newest = past_the_newest_offered(baseline);
    let undeclared = serde_json::json!([{ "key": UNDECLARED_KEY, "value": true }]);

    let mut rows = vec![
        Unacceptable {
            what: "a version one release past the newest it offers".to_string(),
            version: past_newest.clone(),
            parameters: None,
        },
        Unacceptable {
            what: format!("the key {UNDECLARED_KEY}, which no baseline declares"),
            version: newest.clone(),
            parameters: Some(undeclared.clone()),
        },
        Unacceptable {
            what: "a version past the newest it offers AND a key no baseline declares"
                .to_string(),
            version: past_newest,
            parameters: Some(undeclared),
        },
    ];

    for version in every_offered_version() {
        if !baseline.offers_version(version) {
            rows.push(Unacceptable {
                what: format!("{version}, which only another baseline offers"),
                version: version.to_string(),
                parameters: None,
            });
        }
    }

    if let Some(key) = baseline.parameters.first() {
        rows.push(Unacceptable {
            what: format!("the declared key {key} given twice"),
            version: newest,
            parameters: Some(serde_json::json!([
                { "key": key, "value": true },
                { "key": key, "value": false }
            ])),
        });
    }

    rows
}

/// Fire every unacceptable row at the enablement `arn` and hand back, per row, the case it
/// names and the message its refusal carried — asserting each is the modelled
/// `ValidationException` (400) that mints nothing. `resource_state` says what the enablement
/// was doing while the rows were sent, for the assertion messages.
async fn unacceptable_refusals(
    router: &MultiServiceRouter,
    baseline: &CatalogBaseline,
    arn: &str,
    resource_state: &str,
) -> Vec<(String, String)> {
    let mut refusals: Vec<(String, String)> = Vec::new();
    for row in unacceptable_rows(baseline) {
        let case = format!("{} asked for {}", baseline.name, row.what);
        let refused = update(router, arn, &row.version, row.parameters).await;
        assert_refused(&format!("{case} {resource_state}"), &refused, 400, "ValidationException");
        refusals.push((case, refused.string("message")));
    }
    refusals
}

/// Assert the refusals a resource under change answered with are the ones the SAME requests
/// get when nothing is in flight: same rows, same messages.
fn assert_same_refusals(
    settled: &[(String, String)],
    in_flight: &[(String, String)],
    resource_state: &str,
) {
    assert_eq!(
        settled.len(),
        in_flight.len(),
        "case {resource_state:?}: expected one refusal per row, got {} against {}",
        in_flight.len(),
        settled.len()
    );
    for ((case, settled_message), (in_flight_case, in_flight_message)) in
        settled.iter().zip(in_flight)
    {
        assert_eq!(
            case, in_flight_case,
            "case {case:?}: the rows must line up, got {in_flight_case:?}"
        );
        assert_eq!(
            settled_message, in_flight_message,
            "case {case:?} {resource_state}: expected the refusal the same request gets with \
             nothing in flight, {settled_message:?}, got {in_flight_message:?}"
        );
    }
}

/// Assert the enablement `arn` reports itself as still being changed, so the rows fired at
/// it really did overlap an operation in flight.
async fn assert_under_change(router: &MultiServiceRouter, case: &str, arn: &str) {
    let (answer, details) = enabled_baseline_details(router, arn).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: get-enabled-baseline expected 200, got {} body {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        details
            .get("statusSummary")
            .and_then(|summary| member(summary, "status")),
        Some("UNDER_CHANGE"),
        "case {case:?}: expected the enablement under change, got {details:?}"
    );
}

/// Assert that an ACCEPTABLE update of the same enablement is what the conflict answers:
/// a version the baseline offers, with no `parameters` member at all.
async fn assert_conflict(router: &MultiServiceRouter, case: &str, arn: &str, version: &str) {
    let refused = update(router, arn, version, None).await;
    assert_eq!(
        (refused.status, refused.error_code.as_deref()),
        (409, Some("ConflictException")),
        "case {case:?}: expected 409 ConflictException for a request the service would \
         otherwise accept, got {:?} body {:?}",
        (refused.status, refused.error_code.as_deref()),
        refused.body
    );
}

/// POST `/reset-enabled-baseline`, the other call that puts a baseline operation in flight.
async fn reset(router: &MultiServiceRouter, arn: &str) -> Answer {
    call(
        router,
        "/reset-enabled-baseline",
        serde_json::json!({ "enabledBaselineIdentifier": arn }),
    )
    .await
}

/// RULE: `UNDECLARED_KEY` is a key no catalog entry declares, which is what makes every row
/// built from it a request the service never accepts.
#[test]
fn no_catalog_entry_declares_the_undeclared_key() {
    for baseline in BASELINE_CATALOG {
        assert!(
            !baseline.declares_parameter(UNDECLARED_KEY),
            "case {:?}: expected {UNDECLARED_KEY} to be declared by no baseline, got {:?}",
            baseline.name,
            baseline.parameters
        );
    }
    for control in CONTROL_CATALOG {
        assert!(
            !control.declares_parameter(UNDECLARED_KEY),
            "case {:?}: expected {UNDECLARED_KEY} to be declared by no control, got {:?}",
            control.name,
            control.parameters
        );
    }
}

/// RULE (the baseline side): an `UpdateEnabledBaseline` naming a version its enablement's
/// baseline does not offer, or `parameters` that baseline does not accept, is the modelled
/// `ValidationException` (400) — in the same words — whether the enable, an update or a
/// reset is still in flight against that enablement or nothing is; the modelled
/// `ConflictException` (409) answers only a request the service would otherwise accept, and
/// neither refusal writes anything.
#[tokio::test]
async fn an_unacceptable_baseline_update_is_a_bad_request_while_a_change_is_in_flight() {
    let router = set_up_emulator().await;

    // One enablement per catalog baseline, each on its own organizational unit, at the
    // oldest version it offers. Nothing waits for the clock yet, so every ENABLE is still
    // in flight when the first rows are sent.
    let mut enablements: Vec<(&'static CatalogBaseline, String)> = Vec::new();
    for (index, baseline) in BASELINE_CATALOG.iter().enumerate() {
        let enabled = enable(
            &router,
            &served_arn(baseline),
            oldest_offered(baseline),
            &ou(&format!("ro{index}")),
            None,
        )
        .await;
        assert_eq!(
            enabled.status, 200,
            "case {:?}: enable expected 200, got {} body {:?}",
            baseline.name, enabled.status, enabled.body
        );
        enablements.push((baseline, enabled.string("arn")));
    }

    let mut while_enabling: Vec<Vec<(String, String)>> = Vec::new();
    for (baseline, arn) in &enablements {
        assert_under_change(&router, baseline.name, arn).await;
        while_enabling
            .push(unacceptable_refusals(&router, baseline, arn, "while the enable is in flight").await);
        assert_conflict(
            &router,
            &format!("{} while the enable is in flight", baseline.name),
            arn,
            newest_offered(baseline),
        )
        .await;
    }

    tokio::time::sleep(past_the_settle_span()).await;

    // The same rows with nothing in flight: the refusals every in-flight phase is compared
    // with, so no message is spelled out here.
    let mut settled: Vec<Vec<(String, String)>> = Vec::new();
    for (baseline, arn) in &enablements {
        settled.push(unacceptable_refusals(&router, baseline, arn, "with nothing in flight").await);
    }
    for (rows, settled_rows) in while_enabling.iter().zip(&settled) {
        assert_same_refusals(settled_rows, rows, "while the enable is in flight");
    }

    // The same rule while an UPDATE this very call accepted is in flight, and then while a
    // RESET another call started is: the refusal is about the request, so which call put the
    // operation in flight cannot change it.
    for (started_by, start) in [("an update", true), ("a reset", false)] {
        for (baseline, arn) in &enablements {
            let accepted = if start {
                update(&router, arn, newest_offered(baseline), None).await
            } else {
                reset(&router, arn).await
            };
            assert_eq!(
                accepted.status, 200,
                "case {:?}: starting {started_by} expected 200, got {} body {:?}",
                baseline.name, accepted.status, accepted.body
            );
        }

        let resource_state = format!("while {started_by} is in flight");
        for (index, (baseline, arn)) in enablements.iter().enumerate() {
            assert_under_change(&router, baseline.name, arn).await;
            let rows = unacceptable_refusals(&router, baseline, arn, &resource_state).await;
            assert_same_refusals(&settled[index], &rows, &resource_state);
            assert_conflict(
                &router,
                &format!("{} {resource_state}", baseline.name),
                arn,
                oldest_offered(baseline),
            )
            .await;
        }

        tokio::time::sleep(past_the_settle_span()).await;
    }

    // Every refusal above wrote nothing: the applied configuration is the one the accepted
    // update left, re-applied by the accepted reset, and the enablement is settled again.
    for (baseline, arn) in &enablements {
        let (answer, details) = enabled_baseline_details(&router, arn).await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: get-enabled-baseline expected 200, got {} body {:?}",
            baseline.name, answer.status, answer.body
        );
        assert_eq!(
            (
                member(&details, "baselineVersion"),
                details
                    .get("statusSummary")
                    .and_then(|summary| member(summary, "status")),
                applied_parameters(&details)
            ),
            (
                Some(newest_offered(baseline)),
                Some("SUCCEEDED"),
                serde_json::json!([])
            ),
            "case {:?}: only the accepted calls may have applied anything, got {details:?}",
            baseline.name
        );
    }
}

/// POST `/update-enabled-control`; `parameters` is a required member, so every call carries a
/// (possibly empty) list.
async fn update_control(
    router: &MultiServiceRouter,
    identifier: &str,
    parameters: serde_json::Value,
) -> Answer {
    call(
        router,
        "/update-enabled-control",
        serde_json::json!({
            "enabledControlIdentifier": identifier,
            "parameters": parameters,
        }),
    )
    .await
}

/// The unacceptable `parameters` rows for `entry`, derived from its catalog entry: a key no
/// control declares, a key only another control declares, and a key it does declare given
/// twice.
fn unacceptable_control_rows(entry: &CatalogControl) -> Vec<(String, serde_json::Value)> {
    let mut rows = vec![(
        format!("the key {UNDECLARED_KEY}, which no control declares"),
        serde_json::json!([{ "key": UNDECLARED_KEY, "value": true }]),
    )];
    for key in every_declared_control_parameter() {
        if entry.declares_parameter(key) {
            rows.push((
                format!("the declared key {key} given twice"),
                serde_json::json!([
                    { "key": key, "value": true },
                    { "key": key, "value": false }
                ]),
            ));
        } else {
            rows.push((
                format!("{key}, which only another control declares"),
                serde_json::json!([{ "key": key, "value": true }]),
            ));
        }
    }
    rows
}

/// The acceptable `parameters` form for `entry`: every key it declares, once each — the
/// empty list for a control that declares none.
fn declared_control_configuration(entry: &CatalogControl) -> serde_json::Value {
    serde_json::Value::Array(
        entry
            .parameters
            .iter()
            .map(|key| serde_json::json!({ "key": key, "value": "us-west-2" }))
            .collect(),
    )
}

/// Fire every unacceptable control row at `arn`, asserting each is the modelled
/// `ValidationException` (400) that mints nothing, and hand back the case and message of
/// each.
async fn unacceptable_control_refusals(
    router: &MultiServiceRouter,
    entry: &CatalogControl,
    arn: &str,
    resource_state: &str,
) -> Vec<(String, String)> {
    let mut refusals: Vec<(String, String)> = Vec::new();
    for (what, parameters) in unacceptable_control_rows(entry) {
        let case = format!("{} asked for {what}", entry.name);
        let refused = update_control(router, arn, parameters).await;
        assert_refused(&format!("{case} {resource_state}"), &refused, 400, "ValidationException");
        refusals.push((case, refused.string("message")));
    }
    refusals
}

/// RULE (the control side, the same rule): an `UpdateEnabledControl` carrying `parameters`
/// its enablement's control does not declare is the modelled `ValidationException` (400) —
/// in the same words — whether the enable is still in flight against that control or
/// nothing is, while the modelled `ConflictException` (409) answers the same call carrying a
/// configuration the control does declare.
#[tokio::test]
async fn an_unacceptable_control_update_is_a_bad_request_while_a_change_is_in_flight() {
    let router = set_up_emulator().await;

    // One enablement per control a caller may enable, each on its own organizational unit.
    // The enables are asynchronous, so all of them are in flight when the first rows go out.
    let mut enablements: Vec<(&'static CatalogControl, String)> = Vec::new();
    for (index, entry) in CONTROL_CATALOG
        .iter()
        .filter(|entry| entry.enablable)
        .enumerate()
    {
        let (arn, _) = enable_with(
            &router,
            &control(entry.name),
            &control_ou(&format!("rc{index:02}")),
            None,
        )
        .await;
        enablements.push((entry, arn));
    }

    let mut while_enabling: Vec<Vec<(String, String)>> = Vec::new();
    for (entry, arn) in &enablements {
        while_enabling.push(
            unacceptable_control_refusals(&router, entry, arn, "while the enable is in flight")
                .await,
        );

        // The same call with a configuration the control DOES declare is what the conflict
        // answers, so the refusals above are about the request rather than about the control
        // being under change.
        let conflicted = update_control(&router, arn, declared_control_configuration(entry)).await;
        assert_eq!(
            (conflicted.status, conflicted.error_code.as_deref()),
            (409, Some("ConflictException")),
            "case {:?}: expected 409 ConflictException while the enable is in flight, got {:?} \
             body {:?}",
            entry.name,
            (conflicted.status, conflicted.error_code.as_deref()),
            conflicted.body
        );
    }

    tokio::time::sleep(past_the_settle_span()).await;

    for (index, (entry, arn)) in enablements.iter().enumerate() {
        let settled =
            unacceptable_control_refusals(&router, entry, arn, "with nothing in flight").await;
        assert_same_refusals(
            &settled,
            &while_enabling[index],
            "while the enable is in flight",
        );
    }
}
