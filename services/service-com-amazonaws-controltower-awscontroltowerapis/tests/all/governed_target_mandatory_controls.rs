// SPDX-License-Identifier: CC0-1.0

//! The controls AWS Control Tower applies to a GOVERNED target itself, end to end over
//! HTTP.
//!
//! Driven through the same [`MultiServiceRouter`] the production emulator dispatches with,
//! so what these tests exercise is the wire behaviour a client sees — status codes, error
//! codes and JSON bodies — not the handlers' Rust signatures.
//!
//! The rule under test, asserted over TABLES of inputs rather than one case: bringing a
//! target under AWS Control Tower's governance makes the service apply every MANDATORY
//! control of its catalog to that target itself, so the control reporting operations report
//! those enablements for the target beside the ones a caller enabled; a caller may neither
//! enable nor disable one; and ending that governance tears exactly those enablements down
//! again, through the same asynchronous teardown a caller's disable starts.
//!
//! Nothing here writes a control, a baseline or a version out by hand: the mandatory
//! controls are read off `state::CONTROL_CATALOG` (the entries
//! [`CatalogControl::enablable`] is false for, the set `EnableControl` refuses), and the
//! baseline that establishes governance and the versions it offers off
//! `state::BASELINE_CATALOG` — so every case covers entries beyond the ones an issue
//! happened to cite and no row can drift from the catalog it is about. The status and drift
//! value sets come from `smithy_query(query="shape",
//! shape="EnablementStatus,DriftStatus")`, and the errors from
//! `smithy_query(query="operation", operation="DisableControl,EnableControl")`
//! (`ValidationException` is HTTP 400, `ResourceNotFoundException` HTTP 404) on this
//! crate's `smithy-model/`. `EnabledControlFilter$controlIdentifiers` is `@length(1..1)`
//! (`smithy_query(query="shape", shape="EnabledControlFilter")`), so a narrowed row names
//! one identifier at a time.

use std::collections::BTreeSet;

use service_com_amazonaws_controltower_awscontroltowerapis::state::{
    CatalogBaseline, CatalogControl, BASELINE_CATALOG, CONTROL_CATALOG,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::enabled_control_lifecycle::{catalog_arn, control, entries, ou};
use crate::landing_zone_lifecycle::{call, past_the_settle_span, set_up_emulator, Answer};

/// The `BaselineArn` of a catalog baseline, in the region these tests sign for.
pub(crate) fn baseline(id: &str) -> String {
    format!("arn:aws:controltower:us-west-2::baseline/{id}")
}

/// The one catalog baseline whose enablement brings a target under AWS Control Tower's
/// governance, read off the catalog rather than named here.
pub(crate) fn governance_baseline() -> &'static CatalogBaseline {
    let mut governing = BASELINE_CATALOG
        .iter()
        .filter(|baseline| baseline.establishes_governance);
    let governance = governing
        .next()
        .expect("the catalog must publish a baseline that establishes governance");
    assert!(
        governing.next().is_none(),
        "the catalog publishes more than one baseline that establishes governance, so these \
         cases no longer cover the rule"
    );
    governance
}

/// The MANDATORY catalog controls: the entries no caller may enable, which are the ones
/// AWS Control Tower applies to a governed target itself.
pub(crate) fn mandatory() -> Vec<&'static CatalogControl> {
    let mandatory: Vec<&'static CatalogControl> = CONTROL_CATALOG
        .iter()
        .filter(|control| !control.enablable)
        .collect();
    assert!(
        !mandatory.is_empty(),
        "the catalog must publish at least one mandatory control for these cases to be \
         about anything"
    );
    mandatory
}

/// A catalog control a CALLER may enable, so a case can put one beside the mandatory ones.
fn enablable(index: usize) -> &'static CatalogControl {
    let enablable: Vec<&'static CatalogControl> = CONTROL_CATALOG
        .iter()
        .filter(|control| control.enablable)
        .collect();
    assert!(
        !enablable.is_empty(),
        "the catalog must publish at least one control a caller may enable"
    );
    enablable[index % enablable.len()]
}

/// POST `/enable-baseline` for a baseline/version pair on `target`, asserting it succeeded,
/// and hand back the `EnabledBaseline` ARN it minted.
pub(crate) async fn enable_baseline(
    router: &MultiServiceRouter,
    baseline_identifier: &str,
    version: &str,
    target: &str,
) -> String {
    let answer = call(
        router,
        "/enable-baseline",
        serde_json::json!({
            "baselineIdentifier": baseline_identifier,
            "baselineVersion": version,
            "targetIdentifier": target,
        }),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "enable-baseline {baseline_identifier} {version} on {target}: expected 200, got {} \
         body {:?}",
        answer.status, answer.body
    );
    answer.string("arn")
}

/// POST `/list-enabled-controls` with `body`, asserting the listing answered.
pub(crate) async fn list(router: &MultiServiceRouter, body: serde_json::Value) -> Answer {
    let answer = call(router, "/list-enabled-controls", body.clone()).await;
    assert_eq!(
        answer.status, 200,
        "list-enabled-controls {body}: expected 200, got {} body {:?}",
        answer.status, answer.body
    );
    answer
}

/// The `controlIdentifier` of every entry of a listing, sorted, so a case compares SETS
/// without depending on the order ARNs were minted in.
pub(crate) fn listed_controls(listed: &Answer) -> Vec<String> {
    let mut identifiers: Vec<String> = entries(listed.get("enabledControls"))
        .iter()
        .map(|entry| {
            entry
                .get("controlIdentifier")
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    identifiers.sort();
    identifiers
}

/// The enablement ARN a listing reports for `control_identifier`, or the empty string when
/// the listing holds no entry for it.
pub(crate) fn listed_arn_of(listed: &Answer, control_identifier: &str) -> String {
    entries(listed.get("enabledControls"))
        .iter()
        .find(|entry| {
            entry.get("controlIdentifier").and_then(|value| value.as_str())
                == Some(control_identifier)
        })
        .and_then(|entry| entry.get("arn").and_then(|value| value.as_str()))
        .unwrap_or_default()
        .to_string()
}

/// A member of a JSON object, treating an absent member as `Value::Null`.
fn member<'a>(value: &'a serde_json::Value, name: &str) -> &'a serde_json::Value {
    value.get(name).unwrap_or(&serde_json::Value::Null)
}

/// The `statusSummary.status` of a listed entry or of an `EnabledControlDetails`.
fn status(value: &serde_json::Value) -> Option<&str> {
    value
        .get("statusSummary")
        .and_then(|summary| summary.get("status"))
        .and_then(|status| status.as_str())
}

/// The `driftStatusSummary.driftStatus` of a listed entry or of an
/// `EnabledControlDetails`.
fn drift(value: &serde_json::Value) -> Option<&str> {
    value
        .get("driftStatusSummary")
        .and_then(|summary| summary.get("driftStatus"))
        .and_then(|status| status.as_str())
}

/// The AWS Control Tower control identifiers of the mandatory catalog entries, sorted —
/// the set a governed target's listing must report.
fn mandatory_identifiers() -> Vec<String> {
    let mut identifiers: Vec<String> = mandatory()
        .into_iter()
        .map(|entry| control(entry.name))
        .collect();
    identifiers.sort();
    identifiers
}

/// POST `/get-enabled-control` for `identifier`, whatever it answers.
async fn get(router: &MultiServiceRouter, identifier: &str) -> Answer {
    call(
        router,
        "/get-enabled-control",
        serde_json::json!({ "enabledControlIdentifier": identifier }),
    )
    .await
}

/// RULE: bringing a target under AWS Control Tower's governance applies every MANDATORY
/// catalog control to that target, each as its own enablement the control reporting
/// operations report — `UNDER_CHANGE` with `UNKNOWN` drift while the service deploys it,
/// then `SUCCEEDED` and `IN_SYNC` once the clock makes it due — while a baseline that
/// establishes no governance applies none.
///
/// The rows differ along what the rule generalises over: which baseline is enabled (the
/// governing one and every other entry of the catalog), the version it is enabled at (every
/// version that baseline offers) and the target it is enabled on.
#[tokio::test]
async fn governance_applies_every_mandatory_catalog_control_to_its_target() {
    let governance = governance_baseline();
    let expected = mandatory_identifiers();

    // Every version the governing baseline offers, each on a target of its own, then every
    // OTHER catalog baseline at its first offered version: the rows on both sides of
    // `establishes_governance`.
    let mut cases: Vec<(String, String, String, String, bool)> = Vec::new();
    for (index, version) in governance.versions.iter().enumerate() {
        cases.push((
            format!("{} at version {version}", governance.name),
            baseline(governance.id),
            (*version).to_string(),
            ou(&format!("gv{index:02}")),
            true,
        ));
    }
    for (index, entry) in BASELINE_CATALOG
        .iter()
        .filter(|entry| !entry.establishes_governance)
        .enumerate()
    {
        let version = entry
            .versions
            .first()
            .expect("every catalog baseline offers at least one version");
        cases.push((
            format!("{} at version {version}", entry.name),
            baseline(entry.id),
            (*version).to_string(),
            ou(&format!("gw{index:02}")),
            false,
        ));
    }

    let router = set_up_emulator().await;
    let mut governed: Vec<(String, String)> = Vec::new();

    for (case, baseline_identifier, version, target, governs) in cases {
        enable_baseline(&router, &baseline_identifier, &version, &target).await;

        let listed = list(
            &router,
            serde_json::json!({ "targetIdentifier": target, "includeChildren": false }),
        )
        .await;
        let reported = listed_controls(&listed);
        let wanted: Vec<String> = if governs { expected.clone() } else { Vec::new() };
        assert_eq!(
            reported, wanted,
            "case {case:?}: expected the listing of {target} to report {wanted:?}, got \
             {reported:?}"
        );

        if !governs {
            continue;
        }

        // The deployment is asynchronous whoever started it, so the non-terminal phase is
        // asserted HERE, on the target this row just registered: the settle span is short
        // on purpose.
        for identifier in &expected {
            let entry = entries(listed.get("enabledControls"))
                .into_iter()
                .find(|entry| {
                    entry.get("controlIdentifier").and_then(|value| value.as_str())
                        == Some(identifier.as_str())
                })
                .unwrap_or(serde_json::Value::Null);
            assert_eq!(
                (status(&entry), drift(&entry)),
                (Some("UNDER_CHANGE"), Some("UNKNOWN")),
                "case {case:?}, control {identifier}: a control still being applied is \
                 UNDER_CHANGE with UNKNOWN drift, got {entry:?}"
            );

            // The enablement the service applied is a resource of its own, so the ARN the
            // listing reports resolves through the reader a caller's enablement resolves
            // through.
            let arn = listed_arn_of(&listed, identifier);
            let got = get(&router, &arn).await;
            assert_eq!(
                got.status, 200,
                "case {case:?}, control {identifier}: get-enabled-control {arn} expected \
                 200, got {} body {:?}",
                got.status, got.body
            );
            let details = got.get("enabledControlDetails");
            assert_eq!(
                (
                    details.get("controlIdentifier").and_then(|v| v.as_str()),
                    details.get("targetIdentifier").and_then(|v| v.as_str()),
                    details.get("parentIdentifier").and_then(|v| v.as_str()),
                ),
                (Some(identifier.as_str()), Some(target.as_str()), None),
                "case {case:?}, control {identifier}: expected the control applied directly \
                 to {target}, got {details:?}"
            );
            assert_eq!(
                entries(member(details, "parameters")),
                Vec::<serde_json::Value>::new(),
                "case {case:?}, control {identifier}: a mandatory control declares no \
                 parameter, so it carries no configuration, got {details:?}"
            );

            governed.push((case.clone(), arn));
        }
    }

    // Once the clock has made every applied control due, each reports its terminal status
    // and the drift of a deployed configuration.
    tokio::time::sleep(past_the_settle_span()).await;

    for (case, arn) in &governed {
        let got = get(&router, arn).await;
        let details = got.get("enabledControlDetails");
        assert_eq!(
            (got.status, status(details), drift(details)),
            (200, Some("SUCCEEDED"), Some("IN_SYNC")),
            "case {case:?}, enablement {arn}: once settled expected 200 SUCCEEDED IN_SYNC, \
             got {} body {:?}",
            got.status,
            got.body
        );
    }
}

/// RULE (the same rule, from the listing's side): a governed target's listing is the
/// mandatory controls the service applied to it TOGETHER with the ones a caller enabled on
/// it, each listing narrowed by `targetIdentifier` and by `filter` exactly as before — so a
/// target nothing governs reports only what a caller enabled on it.
///
/// The rows differ along what the rule generalises over: a governed target with and without
/// a caller's own enablement, an ungoverned target with one, the listing with no target at
/// all, and the listing narrowed by a `controlIdentifiers` filter naming a mandatory
/// control, an elective one and both.
#[tokio::test]
async fn a_governed_targets_listing_holds_the_mandatory_controls_and_the_callers_own() {
    let governance = governance_baseline();
    let version = governance.versions[0];
    let elective = enablable(0);
    let mandatory_entry = mandatory()[0];

    let governed_bare = ou("lb01");
    let governed_with_own = ou("lb02");
    let ungoverned = ou("lb03");

    let router = set_up_emulator().await;
    enable_baseline(&router, &baseline(governance.id), version, &governed_bare).await;
    enable_baseline(&router, &baseline(governance.id), version, &governed_with_own).await;

    for target in [&governed_with_own, &ungoverned] {
        let enabled = call(
            &router,
            "/enable-control",
            serde_json::json!({
                "controlIdentifier": control(elective.name),
                "targetIdentifier": target,
            }),
        )
        .await;
        assert_eq!(
            enabled.status, 200,
            "enable-control {} on {target}: expected 200, got {} body {:?}",
            elective.name, enabled.status, enabled.body
        );
    }

    let mandatory_set = mandatory_identifiers();
    let mut with_own = mandatory_set.clone();
    with_own.push(control(elective.name));
    with_own.sort();
    let mut everything = with_own.clone();
    everything.extend(mandatory_set.clone());
    everything.push(control(elective.name));
    everything.sort();

    let cases: Vec<(&str, serde_json::Value, Vec<String>)> = vec![
        (
            "a governed target nothing else was enabled on",
            serde_json::json!({ "targetIdentifier": governed_bare }),
            mandatory_set.clone(),
        ),
        (
            "a governed target carrying a caller's own enablement",
            serde_json::json!({ "targetIdentifier": governed_with_own }),
            with_own.clone(),
        ),
        (
            "a target nothing governs, carrying only a caller's own enablement",
            serde_json::json!({ "targetIdentifier": ungoverned }),
            vec![control(elective.name)],
        ),
        (
            "no target at all: every enablement the account holds",
            serde_json::json!({}),
            everything,
        ),
        (
            "narrowed to one mandatory control on a governed target",
            serde_json::json!({
                "targetIdentifier": governed_with_own,
                "filter": { "controlIdentifiers": [control(mandatory_entry.name)] },
            }),
            vec![control(mandatory_entry.name)],
        ),
        (
            "narrowed to one mandatory control by its control catalog ARN",
            serde_json::json!({
                "targetIdentifier": governed_with_own,
                "filter": { "controlIdentifiers": [catalog_arn(mandatory_entry.catalog_id)] },
            }),
            vec![control(mandatory_entry.name)],
        ),
        (
            "narrowed to the elective control a caller enabled",
            serde_json::json!({
                "targetIdentifier": governed_with_own,
                "filter": { "controlIdentifiers": [control(elective.name)] },
            }),
            vec![control(elective.name)],
        ),
        (
            "narrowed to a mandatory control on the target nothing governs",
            serde_json::json!({
                "targetIdentifier": ungoverned,
                "filter": { "controlIdentifiers": [control(mandatory_entry.name)] },
            }),
            Vec::new(),
        ),
    ];

    for (case, body, wanted) in cases {
        let listed = list(&router, body.clone()).await;
        let reported = listed_controls(&listed);
        assert_eq!(
            reported, wanted,
            "case {case:?} ({body}): expected {wanted:?}, got {reported:?}"
        );
    }
}

/// RULE: a MANDATORY control is one AWS Control Tower applies to every target it governs
/// itself, so it is not a caller's to enable OR to disable: every such request is refused
/// with 400 `ValidationException`, whichever way it names the enablement, and the control
/// the service applied is still reported afterwards.
///
/// The rows span every mandatory catalog entry and every way `DisableControl` lets a
/// request name an enablement — its own ARN, the control/target pair under the AWS Control
/// Tower ARN form, and the same pair under the control catalog ARN form — beside
/// `EnableControl` naming the same control on the governed target.
#[tokio::test]
async fn a_mandatory_control_on_a_governed_target_is_not_a_callers_to_enable_or_disable() {
    let governance = governance_baseline();
    let target = ou("mr01");

    let router = set_up_emulator().await;
    enable_baseline(&router, &baseline(governance.id), governance.versions[0], &target).await;
    // Past the settle span, so a refusal below cannot be the in-flight `ConflictException`
    // a control still being applied answers with.
    tokio::time::sleep(past_the_settle_span()).await;

    let listed = list(&router, serde_json::json!({ "targetIdentifier": target })).await;

    for entry in mandatory() {
        let identifier = control(entry.name);
        let arn = listed_arn_of(&listed, &identifier);
        assert!(
            !arn.is_empty(),
            "control {}: the governed target must report the control the service applied, \
             got {:?}",
            entry.name,
            listed.body
        );

        let requests: Vec<(String, &str, serde_json::Value)> = vec![
            (
                format!("{}: enable-control naming it", entry.name),
                "/enable-control",
                serde_json::json!({
                    "controlIdentifier": identifier,
                    "targetIdentifier": target,
                }),
            ),
            (
                format!("{}: disable-control by the enablement's ARN", entry.name),
                "/disable-control",
                serde_json::json!({ "enabledControlIdentifier": arn }),
            ),
            (
                format!("{}: disable-control by the control/target pair", entry.name),
                "/disable-control",
                serde_json::json!({
                    "controlIdentifier": identifier,
                    "targetIdentifier": target,
                }),
            ),
            (
                format!("{}: disable-control by the catalog ARN pair", entry.name),
                "/disable-control",
                serde_json::json!({
                    "controlIdentifier": catalog_arn(entry.catalog_id),
                    "targetIdentifier": target,
                }),
            ),
        ];

        for (case, path, body) in requests {
            let refused = call(&router, path, body.clone()).await;
            assert_eq!(
                (refused.status, refused.error_code.as_deref()),
                (400, Some("ValidationException")),
                "case {case:?} ({body}): expected 400 ValidationException, got {:?} body {:?}",
                (refused.status, refused.error_code.as_deref()),
                refused.body
            );
            assert!(
                refused.body.get("operationIdentifier").is_none(),
                "case {case:?}: a refusal must start no operation, got {:?}",
                refused.body
            );
        }

        // No refusal touched the control the service applied.
        let got = get(&router, &arn).await;
        let details = got.get("enabledControlDetails");
        assert_eq!(
            (got.status, status(details)),
            (200, Some("SUCCEEDED")),
            "control {}: after the refusals the applied control must still be SUCCEEDED, \
             got {} body {:?}",
            entry.name,
            got.status,
            got.body
        );
    }
}

/// RULE: ending a target's governance withdraws exactly the controls the service applied to
/// it — each through the asynchronous teardown a caller's disable starts, so they are
/// `UNDER_CHANGE` until the clock makes it due and gone afterwards — while an enablement a
/// CALLER made on that target is the caller's and survives, and re-governing the target
/// applies them again.
///
/// The rows differ along what the rule generalises over: the phase of the teardown (started,
/// then due), which enablements it reaches (the service's on that target, not the caller's
/// and not another target's) and governance withdrawn then re-established.
#[tokio::test]
async fn withdrawing_governance_tears_down_the_controls_it_applied() {
    let governance = governance_baseline();
    let version = governance.versions[0];
    let elective = enablable(0);
    let withdrawn = ou("wd01");
    let kept = ou("wd02");

    let router = set_up_emulator().await;
    let enablement = enable_baseline(&router, &baseline(governance.id), version, &withdrawn).await;
    // A second governed target, so the teardown is shown to reach the withdrawn target's
    // controls only.
    enable_baseline(&router, &baseline(governance.id), version, &kept).await;
    let own = call(
        &router,
        "/enable-control",
        serde_json::json!({
            "controlIdentifier": control(elective.name),
            "targetIdentifier": withdrawn,
        }),
    )
    .await;
    assert_eq!(
        own.status, 200,
        "enable-control {} on {withdrawn}: expected 200, got {} body {:?}",
        elective.name, own.status, own.body
    );
    let own_arn = own.string("arn");
    tokio::time::sleep(past_the_settle_span()).await;

    let before = list(&router, serde_json::json!({ "targetIdentifier": withdrawn })).await;
    let applied: Vec<(String, String)> = mandatory()
        .into_iter()
        .map(|entry| {
            let identifier = control(entry.name);
            (identifier.clone(), listed_arn_of(&before, &identifier))
        })
        .collect();

    let disabled = call(
        &router,
        "/disable-baseline",
        serde_json::json!({ "enabledBaselineIdentifier": enablement }),
    )
    .await;
    assert_eq!(
        disabled.status, 200,
        "disable-baseline {enablement}: expected 200, got {} body {:?}",
        disabled.status, disabled.body
    );

    // The teardown is asynchronous, so a caller polling straight after the call still sees
    // every withdrawn control, UNDER_CHANGE.
    let during = list(&router, serde_json::json!({ "targetIdentifier": withdrawn })).await;
    for (identifier, arn) in &applied {
        let entry = entries(during.get("enabledControls"))
            .into_iter()
            .find(|entry| entry.get("arn").and_then(|value| value.as_str()) == Some(arn.as_str()))
            .unwrap_or(serde_json::Value::Null);
        assert_eq!(
            (status(&entry), drift(&entry)),
            (Some("UNDER_CHANGE"), Some("UNKNOWN")),
            "control {identifier}: a control being withdrawn is UNDER_CHANGE with UNKNOWN \
             drift until the clock makes the teardown due, got {entry:?}"
        );
    }

    tokio::time::sleep(past_the_settle_span()).await;

    // Once due: the controls the service applied are gone, and only the caller's own
    // enablement is left on that target.
    let after = list(&router, serde_json::json!({ "targetIdentifier": withdrawn })).await;
    assert_eq!(
        listed_controls(&after),
        vec![control(elective.name)],
        "once the teardown is due only the caller's own enablement is left on {withdrawn}, \
         got {:?}",
        after.body
    );
    for (identifier, arn) in &applied {
        let got = get(&router, arn).await;
        assert_eq!(
            (got.status, got.error_code.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "control {identifier}: a withdrawn enablement no longer resolves, got {:?} body \
             {:?}",
            (got.status, got.error_code.as_deref()),
            got.body
        );
    }
    let still = get(&router, &own_arn).await;
    assert_eq!(
        (still.status, status(still.get("enabledControlDetails"))),
        (200, Some("SUCCEEDED")),
        "the caller's own enablement is the caller's to tear down, so it survives the \
         withdrawal, got {} body {:?}",
        still.status,
        still.body
    );

    // The target that stayed governed kept every control the service applied to it.
    let other = list(&router, serde_json::json!({ "targetIdentifier": kept })).await;
    assert_eq!(
        listed_controls(&other),
        mandatory_identifiers(),
        "the withdrawal must not reach another governed target, got {:?}",
        other.body
    );

    // Governing the target again applies them again, as fresh enablements: the set is the
    // catalog's, and no ARN the withdrawal removed comes back.
    enable_baseline(&router, &baseline(governance.id), version, &withdrawn).await;
    let again = list(&router, serde_json::json!({ "targetIdentifier": withdrawn })).await;
    let mut wanted = mandatory_identifiers();
    wanted.push(control(elective.name));
    wanted.sort();
    assert_eq!(
        listed_controls(&again),
        wanted,
        "re-governing {withdrawn} applies the mandatory controls again beside the caller's \
         own, got {:?}",
        again.body
    );
    let removed: BTreeSet<&str> = applied.iter().map(|(_, arn)| arn.as_str()).collect();
    for entry in entries(again.get("enabledControls")) {
        let arn = entry
            .get("arn")
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_string();
        assert!(
            !removed.contains(arn.as_str()),
            "a re-applied control is a fresh enablement, so it must not reuse the ARN {arn} \
             the withdrawal removed, got {entry:?}"
        );
    }
}
