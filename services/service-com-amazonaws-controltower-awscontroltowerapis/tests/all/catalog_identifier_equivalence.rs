// SPDX-License-Identifier: CC0-1.0

//! Identifying a CATALOG resource, end to end over HTTP: `EnableBaseline`,
//! `EnableControl` and the listings that filter on the resource they name.
//!
//! Driven through the same [`MultiServiceRouter`] the production emulator dispatches
//! with, so what these tests exercise is the wire behaviour a client sees — status codes,
//! error codes and JSON bodies — not the handlers' Rust signatures.
//!
//! The rule under test, asserted over TABLES of inputs rather than one case: a baseline
//! and a control are identified by the CATALOG ENTRY their identifier resolves to, not by
//! that identifier's text, so every identifier that resolves to one entry names that one
//! resource — the one-per-target enablement rule refuses a repeat written with any of
//! them, and a listing's identifier filter selects an enablement named by any of them.
//!
//! The equivalent forms are the ones the modelled identifier types admit, each named at
//! the table that builds it: `BaselineArn`'s pattern
//! `^arn:[a-z-]+:controltower:[a-z0-9-]*:[0-9]{0,12}:baseline/[A-Z0-9]{16}$` and the two
//! published control ARN forms, from
//! `smithy_query(query="shape", shape="BaselineArn,ControlIdentifier")` on this crate's
//! `smithy-model/`; every row is also inside `Arn`'s own
//! `^arn:aws[0-9a-zA-Z_\-:\/]+$` (length 20..2048), which is what the front door of these
//! operations checks. The baselines and controls themselves come from
//! `service_..._awscontroltowerapis::state::{BASELINE_CATALOG, CONTROL_CATALOG}` — the
//! catalogs `ListBaselines` and `EnableControl` resolve against — so the cases cover
//! entries beyond the one an issue happened to cite.

use service_com_amazonaws_controltower_awscontroltowerapis::state::{
    CatalogBaseline, CatalogControl, BASELINE_CATALOG, CONTROL_CATALOG,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::enabled_control_lifecycle::entries;
use crate::landing_zone_lifecycle::{call, past_the_settle_span, set_up_emulator, Answer};

/// An organizational-unit target, in the form the AWS Organizations documentation
/// publishes. `suffix` keeps the rows of one case on distinct targets.
pub(crate) fn ou(suffix: &str) -> String {
    format!("arn:aws:organizations::123456789012:ou/o-exampleorg/ou-eq11-{suffix}")
}

/// Every identifier that names the catalog baseline `baseline_id`, as `BaselineArn`'s own
/// form admits them: the partition, the region and the account segment of a baseline ARN
/// select nothing — the same catalog is published in every partition, region and account
/// — so these differ in exactly the segments that carry no identity, and in nothing else.
pub(crate) fn baseline_identifier_forms(baseline_id: &str) -> Vec<(String, String)> {
    [
        (
            "the region-flavoured form GetBaseline reports",
            format!("arn:aws:controltower:us-west-2::baseline/{baseline_id}"),
        ),
        (
            "another region",
            format!("arn:aws:controltower:us-east-1::baseline/{baseline_id}"),
        ),
        (
            "no region at all, which the pattern's empty region segment admits",
            format!("arn:aws:controltower:::baseline/{baseline_id}"),
        ),
        (
            "an account named in the account segment",
            format!("arn:aws:controltower::123456789012:baseline/{baseline_id}"),
        ),
        (
            "a region and an account together",
            format!("arn:aws:controltower:eu-central-1:000000000000:baseline/{baseline_id}"),
        ),
        (
            "another partition",
            format!("arn:aws-us-gov:controltower:us-gov-west-1::baseline/{baseline_id}"),
        ),
    ]
    .into_iter()
    .map(|(case, identifier)| (case.to_string(), identifier))
    .collect()
}

/// Every identifier that names the catalog control `control`: the AWS Control Tower form,
/// whose resource segment is the control's name, and the AWS Control Catalog form, whose
/// resource segment is the control's opaque id — in several partitions and regions, which
/// a control ARN carries no identity in either.
pub(crate) fn control_identifier_forms(control: &CatalogControl) -> Vec<(String, String)> {
    [
        (
            "the AWS Control Tower form, in the request's region",
            format!("arn:aws:controltower:us-west-2::control/{}", control.name),
        ),
        (
            "the AWS Control Tower form, in another region",
            format!("arn:aws:controltower:eu-west-1::control/{}", control.name),
        ),
        (
            "the AWS Control Tower form with no region",
            format!("arn:aws:controltower:::control/{}", control.name),
        ),
        (
            "the AWS Control Tower form in another partition",
            format!(
                "arn:aws-us-gov:controltower:us-gov-west-1::control/{}",
                control.name
            ),
        ),
        (
            "the AWS Control Catalog form",
            format!("arn:aws:controlcatalog:::control/{}", control.catalog_id),
        ),
        (
            "the AWS Control Catalog form in another partition",
            format!("arn:aws-cn:controlcatalog:::control/{}", control.catalog_id),
        ),
    ]
    .into_iter()
    .map(|(case, identifier)| (case.to_string(), identifier))
    .collect()
}

/// The oldest and the newest version a baseline offers, so a repeat can be written at
/// another version than the enablement it repeats: the version is not part of what
/// identifies an enablement.
pub(crate) fn oldest_and_newest(baseline: &CatalogBaseline) -> (&'static str, &'static str) {
    let oldest = baseline
        .versions
        .first()
        .expect("a catalog baseline offers at least one version");
    let newest = baseline
        .versions
        .last()
        .expect("a catalog baseline offers at least one version");
    (oldest, newest)
}

/// POST `/enable-baseline` for a baseline/target pair at `version`.
pub(crate) async fn enable_baseline(
    router: &MultiServiceRouter,
    baseline_identifier: &str,
    version: &str,
    target: &str,
) -> Answer {
    call(
        router,
        "/enable-baseline",
        serde_json::json!({
            "baselineIdentifier": baseline_identifier,
            "baselineVersion": version,
            "targetIdentifier": target,
        }),
    )
    .await
}

/// POST `/enable-control` for a control/target pair.
pub(crate) async fn enable_control(
    router: &MultiServiceRouter,
    control_identifier: &str,
    target: &str,
) -> Answer {
    call(
        router,
        "/enable-control",
        serde_json::json!({
            "controlIdentifier": control_identifier,
            "targetIdentifier": target,
        }),
    )
    .await
}

/// POST `/list-enabled-baselines` with `body`.
async fn list_enabled_baselines(router: &MultiServiceRouter, body: serde_json::Value) -> Answer {
    call(router, "/list-enabled-baselines", body).await
}

/// The `arn` of every entry of a listing of `member`, sorted, so a case compares SETS
/// without depending on the order the ARNs were minted in.
pub(crate) fn listed_arns(listed: &Answer, member: &str) -> Vec<String> {
    let mut arns: Vec<String> = entries(listed.get(member))
        .iter()
        .map(|entry| {
            entry
                .get("arn")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    arns.sort();
    arns
}

/// Assert that `refused` carries the status and error code the rule names, and that it
/// minted nothing: a refused enablement reports neither an ARN nor an operation.
pub(crate) fn assert_refused(case: &str, refused: &Answer, status: u16, code: &str) {
    assert_eq!(
        (refused.status, refused.error_code.as_deref()),
        (status, Some(code)),
        "case {case:?}: expected {status} {code}, got {:?} body {:?}",
        (refused.status, refused.error_code.as_deref()),
        refused.body
    );
    assert!(
        refused.body.get("arn").is_none() && refused.body.get("operationIdentifier").is_none(),
        "case {case:?}: a refused enablement must mint nothing, got {:?}",
        refused.body
    );
}

/// A 16-character baseline resource id the catalog does NOT hold, built off `seed` by
/// replacing its last character — so the case tracks the catalog rather than hard-coding
/// a value that could one day become a real entry.
fn uncatalogued_baseline_id(seed: &str) -> String {
    let replacement = if seed.ends_with('X') { 'Y' } else { 'X' };
    let mut id: String = seed.chars().take(15).collect();
    id.push(replacement);
    assert!(
        !BASELINE_CATALOG.iter().any(|baseline| baseline.id == id),
        "the uncatalogued id {id:?} must not be a catalog entry"
    );
    id
}

/// RULE: a baseline is identified by the catalog entry its identifier resolves to, so one
/// baseline is enabled at most once per target whichever of the equivalent identifiers
/// names it — and whatever version the repeat asks for, in flight or already deployed.
#[tokio::test]
async fn one_baseline_enables_once_per_target_whichever_identifier_names_it() {
    let router = set_up_emulator().await;
    let mut enabled: Vec<(String, String)> = Vec::new();

    for (index, baseline) in BASELINE_CATALOG.iter().enumerate() {
        let target = ou(&format!("b{index}"));
        let (oldest, newest) = oldest_and_newest(baseline);
        let forms = baseline_identifier_forms(baseline.id);
        let (first_case, first_form) = &forms[0];

        let first = enable_baseline(&router, first_form, oldest, &target).await;
        assert_eq!(
            first.status, 200,
            "case {:?} ({first_case}): expected 200, got {} body {:?}",
            baseline.name, first.status, first.body
        );
        enabled.push((baseline.name.to_string(), first.string("arn")));

        // Every other identifier of the SAME baseline on the same target is the same
        // pair, so it is refused — at the newest offered version, to show the version the
        // repeat asks for is not part of what identifies the enablement.
        for (case, form) in &forms[1..] {
            let again = enable_baseline(&router, form, newest, &target).await;
            assert_refused(
                &format!("{} repeated with {case}", baseline.name),
                &again,
                409,
                "ConflictException",
            );
        }
    }

    // The store holds exactly the accepted enablements: the refusals wrote nothing.
    let listed = list_enabled_baselines(&router, serde_json::json!({})).await;
    let mut expected: Vec<String> = enabled.iter().map(|(_, arn)| arn.clone()).collect();
    expected.sort();
    assert_eq!(
        listed_arns(&listed, "enabledBaselines"),
        expected,
        "a refused enablement must persist nothing, got {:?}",
        listed.body
    );

    // Once the enablement is deployed rather than in flight, the pair is still taken —
    // and the refusal is now the one for a request that asks for something that already
    // exists, rather than the in-flight conflict above.
    tokio::time::sleep(past_the_settle_span()).await;

    for (index, baseline) in BASELINE_CATALOG.iter().enumerate() {
        let target = ou(&format!("b{index}"));
        let (_, newest) = oldest_and_newest(baseline);
        let forms = baseline_identifier_forms(baseline.id);
        let (case, form) = forms
            .last()
            .expect("the form table carries more than one row");
        let again = enable_baseline(&router, form, newest, &target).await;
        assert_refused(
            &format!("{} deployed, repeated with {case}", baseline.name),
            &again,
            400,
            "ValidationException",
        );
    }
}

/// RULE (the other side of it): the rule is per baseline AND target, so an equivalent
/// identifier of the same baseline enables on ANOTHER target, another baseline enables on
/// the same target, and the pair enables again — under any of its identifiers — once its
/// teardown is due.
#[tokio::test]
async fn the_pair_rule_narrows_to_one_baseline_and_one_target() {
    let [first, second] = [0usize, 1]
        .map(|index| BASELINE_CATALOG.get(index).expect("two catalog baselines"));
    let (version, _) = oldest_and_newest(first);
    let (second_version, _) = oldest_and_newest(second);
    let first_forms = baseline_identifier_forms(first.id);
    let second_forms = baseline_identifier_forms(second.id);
    let target = ou("p0");
    let other_target = ou("p1");

    let router = set_up_emulator().await;
    let enabled = enable_baseline(&router, &first_forms[0].1, version, &target).await;
    assert_eq!(
        enabled.status, 200,
        "the first enablement: expected 200, got {} body {:?}",
        enabled.status, enabled.body
    );
    let arn = enabled.string("arn");

    for (case, body) in [
        (
            "the same baseline, named by another identifier, on another target",
            (&first_forms[2].1, version, &other_target),
        ),
        (
            "another baseline, named by another identifier, on the same target",
            (&second_forms[3].1, second_version, &target),
        ),
    ] {
        let (identifier, asked_version, on_target) = body;
        let accepted = enable_baseline(&router, identifier, asked_version, on_target).await;
        assert_eq!(
            accepted.status, 200,
            "case {case:?}: expected 200, got {} body {:?}",
            accepted.status, accepted.body
        );
        assert_ne!(
            accepted.string("arn"),
            arn,
            "case {case:?}: a different pair is a different enablement, got {arn:?} twice"
        );
    }

    tokio::time::sleep(past_the_settle_span()).await;

    let disabled = call(
        &router,
        "/disable-baseline",
        serde_json::json!({ "enabledBaselineIdentifier": arn }),
    )
    .await;
    assert_eq!(
        disabled.status, 200,
        "the teardown: expected 200, got {} body {:?}",
        disabled.status, disabled.body
    );

    tokio::time::sleep(past_the_settle_span()).await;

    // The pair is free again, and every identifier of that baseline enables it again —
    // one at a time, so the second of them is refused once more.
    let again = enable_baseline(&router, &first_forms[4].1, version, &target).await;
    assert_eq!(
        again.status, 200,
        "re-enabling the torn-down pair: expected 200, got {} body {:?}",
        again.status, again.body
    );
    assert_ne!(
        again.string("arn"),
        arn,
        "re-enabling mints a new EnabledBaseline, got the torn-down {arn:?}"
    );
    let third = enable_baseline(&router, &first_forms[1].1, version, &target).await;
    assert_refused(
        "a third enable of the re-enabled pair",
        &third,
        409,
        "ConflictException",
    );
}

/// RULE: `ListEnabledBaselines`' `filter.baselineIdentifiers` selects on the baseline a
/// value resolves to, so it finds every enablement of that baseline whichever identifier
/// the enabling call named it by — and a value naming no catalog baseline selects nothing.
#[tokio::test]
async fn a_baseline_filter_selects_enablements_named_by_any_identifier_of_that_baseline() {
    let first = BASELINE_CATALOG.first().expect("a catalog baseline");
    let second = BASELINE_CATALOG.get(1).expect("a second catalog baseline");
    let (version, _) = oldest_and_newest(first);
    let (second_version, _) = oldest_and_newest(second);
    let first_forms = baseline_identifier_forms(first.id);
    let second_forms = baseline_identifier_forms(second.id);

    let router = set_up_emulator().await;

    // One enablement of the first baseline per equivalent identifier, each on its own
    // target, so the stored identifiers span every form.
    let mut of_first: Vec<String> = Vec::new();
    for (index, (case, form)) in first_forms.iter().enumerate() {
        let enabled = enable_baseline(&router, form, version, &ou(&format!("f{index}"))).await;
        assert_eq!(
            enabled.status, 200,
            "case {case:?}: expected 200, got {} body {:?}",
            enabled.status, enabled.body
        );
        of_first.push(enabled.string("arn"));
    }
    of_first.sort();

    let other = enable_baseline(&router, &second_forms[2].1, second_version, &ou("f9")).await;
    assert_eq!(
        other.status, 200,
        "the other baseline's enablement: expected 200, got {} body {:?}",
        other.status, other.body
    );
    let of_second = vec![other.string("arn")];

    let mut everything: Vec<String> = of_first.iter().cloned().chain(of_second.clone()).collect();
    everything.sort();

    // A baseline ARN of the right form naming no catalog entry: it resolves to no
    // baseline, so it selects nothing.
    let uncatalogued = format!(
        "arn:aws:controltower:us-west-2::baseline/{}",
        uncatalogued_baseline_id(first.id)
    );

    let mut cases: Vec<(String, serde_json::Value, Vec<String>)> = Vec::new();
    for (case, form) in &first_forms {
        cases.push((
            format!("the first baseline, filtered by {case}"),
            serde_json::json!({ "filter": { "baselineIdentifiers": [form] } }),
            of_first.clone(),
        ));
    }
    for (case, form) in &second_forms {
        cases.push((
            format!("the second baseline, filtered by {case}"),
            serde_json::json!({ "filter": { "baselineIdentifiers": [form] } }),
            of_second.clone(),
        ));
    }
    cases.push((
        "both baselines, each written in a different form".to_string(),
        serde_json::json!({
            "filter": { "baselineIdentifiers": [first_forms[5].1, second_forms[0].1] }
        }),
        everything.clone(),
    ));
    cases.push((
        "a baseline ARN naming no catalog baseline".to_string(),
        serde_json::json!({ "filter": { "baselineIdentifiers": [uncatalogued] } }),
        Vec::new(),
    ));
    cases.push((
        "no filter at all".to_string(),
        serde_json::json!({}),
        everything.clone(),
    ));

    for (case, body, expected) in cases {
        let listed = list_enabled_baselines(&router, body.clone()).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?} ({body}): expected 200, got {} body {:?}",
            listed.status, listed.body
        );
        assert_eq!(
            listed_arns(&listed, "enabledBaselines"),
            expected,
            "case {case:?} ({body}): expected {expected:?}, got {:?}",
            listed.body
        );
    }
}

/// RULE (the same rule on the control side): a control is identified by the catalog entry
/// its identifier resolves to — either published ARN form, in any partition or region —
/// so one control is enabled at most once per target whichever identifier names it, and
/// `ListEnabledControls`/`ListControlOperations` find that enablement by any of them.
#[tokio::test]
async fn one_control_enables_once_per_target_whichever_identifier_names_it() {
    let router = set_up_emulator().await;
    let enablable: Vec<&CatalogControl> = CONTROL_CATALOG
        .iter()
        .filter(|control| control.enablable)
        .collect();
    assert!(
        enablable.len() > 1,
        "the catalog must publish more than one enablable control, got {}",
        enablable.len()
    );

    let mut enabled: Vec<(&CatalogControl, String, String, String)> = Vec::new();
    for (index, control) in enablable.iter().enumerate() {
        let target = ou(&format!("c{index}"));
        let forms = control_identifier_forms(control);
        // The enabling call rotates through the forms, so no case depends on the
        // enablement having been written under the form a later filter asks by.
        let (case, form) = &forms[index % forms.len()];

        let accepted = enable_control(&router, form, &target).await;
        assert_eq!(
            accepted.status, 200,
            "case {:?} ({case}): expected 200, got {} body {:?}",
            control.name, accepted.status, accepted.body
        );
        let arn = accepted.string("arn");
        let operation_identifier = accepted.string("operationIdentifier");

        for (repeat_case, repeat_form) in forms.iter().filter(|(_, other)| other != form) {
            let again = enable_control(&router, repeat_form, &target).await;
            assert_refused(
                &format!("{} repeated with {repeat_case} while deploying", control.name),
                &again,
                409,
                "ConflictException",
            );
        }

        enabled.push((control, arn, operation_identifier, target));
    }

    // Every identifier of a control finds the enablement the listings hold for it, and
    // the operation record that enabling call wrote.
    for (control, arn, operation_identifier, target) in &enabled {
        let forms = control_identifier_forms(control);
        for (case, form) in &forms {
            let listed = call(
                &router,
                "/list-enabled-controls",
                serde_json::json!({
                    "targetIdentifier": target,
                    "filter": { "controlIdentifiers": [form] }
                }),
            )
            .await;
            assert_eq!(
                listed_arns(&listed, "enabledControls"),
                vec![arn.clone()],
                "case {:?} filtered by {case}: expected [{arn:?}], got {:?}",
                control.name,
                listed.body
            );

            let operations = call(
                &router,
                "/list-control-operations",
                serde_json::json!({ "filter": { "controlIdentifiers": [form] } }),
            )
            .await;
            let identifiers: Vec<String> = entries(operations.get("controlOperations"))
                .iter()
                .map(|record| {
                    record
                        .get("operationIdentifier")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect();
            assert_eq!(
                identifiers,
                vec![operation_identifier.clone()],
                "case {:?}: the operation log filtered by {case}: expected \
                 [{operation_identifier:?}], got {:?}",
                control.name,
                operations.body
            );
        }
    }

    // Once deployed, a repeat of the pair asks for something that already exists, which
    // the handler refuses as a bad request rather than a conflict.
    tokio::time::sleep(past_the_settle_span()).await;

    for (control, _, _, target) in &enabled {
        let forms = control_identifier_forms(control);
        for (case, form) in [&forms[0], &forms[4]] {
            let again = enable_control(&router, form, target).await;
            assert_refused(
                &format!("{} deployed, repeated with {case}", control.name),
                &again,
                400,
                "ValidationException",
            );
        }
    }
}
