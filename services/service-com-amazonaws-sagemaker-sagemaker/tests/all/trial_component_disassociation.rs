// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for SageMaker's `DisassociateTrialComponent`.
//!
//! Every case goes over the wire (`POST /` with
//! `X-Amz-Target: SageMaker.DisassociateTrialComponent`, `awsJson1_1`) through the same
//! `MultiServiceRouter` the production router uses, so what these tests exercise is what a
//! real SDK client gets. The harness is the one `inference_deployment_lifecycle` declares,
//! not a second copy of it.
//!
//! The tests assert RULES over a TABLE of inputs rather than one recorded call: the one
//! membership the call drops, read back through `ListTrialComponents`' `TrialName` and
//! `ExperimentName` filters; the stored ARNs it answers for either end, whatever Region
//! each end was created in and whichever Region the call signs for; that only the named
//! pair's edge goes, so the other trials the component is part of and the other components
//! of the trial are untouched and both resources themselves survive; the case folding
//! `ExperimentEntityName` documents; that a pair that is not recorded has nothing to drop;
//! and the modelled `ResourceNotFound` for either end the account does not hold.

use serde_json::{json, Value};
use service_com_amazonaws_sagemaker_sagemaker::ComAmazonawsSagemakerSagemakerState;

use crate::inference_deployment_lifecycle::{error_code, message, str_field, Emulator, ACCOUNT};

/// The ARN resource kind `TrialComponentArn` is modelled with
/// (`^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:experiment-trial-component/`,
/// smithy_query operation=DisassociateTrialComponent), spelled out here so the test pins
/// the wire form rather than echoing the constant under test.
const COMPONENT_KIND: &str = "experiment-trial-component";

/// The ARN resource kind `TrialArn` is modelled with
/// (`^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:experiment-trial/`, smithy_query
/// operation=DisassociateTrialComponent). See [`COMPONENT_KIND`].
const TRIAL_KIND: &str = "experiment-trial";

/// A name of exactly `length` characters that satisfies `ExperimentEntityName`'s pattern
/// `^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,119}$` (`len 1..120`, smithy_query
/// operation=DisassociateTrialComponent).
fn name_of_length(length: usize) -> String {
    "c".repeat(length)
}

/// Create the trial `trial` under the experiment `experiment` in `region`, creating the
/// experiment too, so one call seeds a trial nothing else shares an experiment with.
async fn seed_trial(emulator: &Emulator, region: &str, experiment: &str, trial: &str) {
    seed_experiment(emulator, region, experiment).await;
    seed_trial_under(emulator, region, experiment, trial).await;
}

/// Create the experiment `experiment` in `region`.
async fn seed_experiment(emulator: &Emulator, region: &str, experiment: &str) {
    let (status, created) = emulator
        .call_in(region, "CreateExperiment", &json!({ "ExperimentName": experiment }))
        .await;
    assert_eq!(
        status, 200,
        "seeding experiment {experiment}: expected HTTP 200, got {status} with {created:?}"
    );
}

/// Create the trial `trial` under an experiment that already exists.
async fn seed_trial_under(emulator: &Emulator, region: &str, experiment: &str, trial: &str) {
    let (status, created) = emulator
        .call_in(
            region,
            "CreateTrial",
            &json!({ "TrialName": trial, "ExperimentName": experiment }),
        )
        .await;
    assert_eq!(
        status, 200,
        "seeding trial {trial}: expected HTTP 200, got {status} with {created:?}"
    );
}

/// Create the trial component `component` in `region`.
async fn seed_component(emulator: &Emulator, region: &str, component: &str) {
    let (status, created) = emulator
        .call_in(region, "CreateTrialComponent", &json!({ "TrialComponentName": component }))
        .await;
    assert_eq!(
        status, 200,
        "seeding component {component}: expected HTTP 200, got {status} with {created:?}"
    );
}

/// Record the membership of `component` in `trial`, signing for `region`.
async fn associate_in(emulator: &Emulator, region: &str, trial: &str, component: &str) {
    let (status, associated) = emulator
        .call_in(
            region,
            "AssociateTrialComponent",
            &json!({ "TrialName": trial, "TrialComponentName": component }),
        )
        .await;
    assert_eq!(
        status, 200,
        "seeding the membership ({trial}, {component}): expected HTTP 200, got {status} \
         with {associated:?}"
    );
}

/// The `TrialComponentName`s `ListTrialComponents` reports for `filter`, in response order.
async fn listed_names(emulator: &Emulator, filter: &Value) -> Vec<String> {
    let listed = emulator.ok("ListTrialComponents", filter).await;
    listed
        .get("TrialComponentSummaries")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected a TrialComponentSummaries list in {listed}"))
        .iter()
        .map(|summary| str_field(summary, "TrialComponentName"))
        .collect()
}

/// How many memberships the shared store holds, so a refusal or a no-op can be shown to
/// have changed nothing at all rather than merely nothing a listing happens to report.
fn membership_count(emulator: &Emulator) -> usize {
    let state = emulator
        .engine
        .state::<ComAmazonawsSagemakerSagemakerState>();
    // The guard is a named local so it is dropped before `state`, the handle it borrows.
    let store = state.lock();
    store.trial_component_trials.len()
}

/// RULE: `DisassociateTrialComponent` drops the named component's membership of the named
/// trial — the membership `ListTrialComponents`' `TrialName` filter reports — and answers
/// the STORED ARN of each end, so each ARN names the Region its own resource was created in
/// rather than the Region this call signed for; both resources themselves survive.
#[tokio::test]
async fn disassociating_drops_the_membership_and_answers_both_stored_arns() {
    // Rows differ along what the rule generalises over: the name shapes of both ends
    // (single character, digits, hyphens, mixed case, each published length bound), the
    // Region each end was created in, the Region the associate signed for and the Region
    // the disassociate signs for — including rows where they all differ.
    let cases: Vec<(&str, String, String, &str, &str, &str, &str)> = vec![
        (
            "same-region",
            "trial-one".to_string(),
            "component-one".to_string(),
            "us-east-1",
            "us-east-1",
            "us-east-1",
            "us-east-1",
        ),
        (
            "name-length-lower-bound",
            "t".to_string(),
            name_of_length(1),
            "us-west-2",
            "us-west-2",
            "us-west-2",
            "us-west-2",
        ),
        (
            "name-length-upper-bound",
            "t".repeat(120),
            name_of_length(120),
            "eu-west-3",
            "eu-west-3",
            "eu-west-3",
            "eu-west-3",
        ),
        (
            "digits-and-mixed-case",
            "Trial9000".to_string(),
            "Component9000".to_string(),
            "ap-southeast-2",
            "ap-southeast-2",
            "ap-southeast-2",
            "ap-southeast-2",
        ),
        (
            "every-region-differs",
            "trial-cross-region".to_string(),
            "component-cross-region".to_string(),
            "ca-central-1",
            "eu-west-3",
            "us-east-1",
            "ap-south-1",
        ),
    ];

    for (case, trial, component, trial_region, component_region, associate_region, signing_region) in
        &cases
    {
        let emulator = Emulator::new();
        seed_trial(&emulator, trial_region, "experiment-for-the-trial", trial).await;
        seed_component(&emulator, component_region, component).await;
        associate_in(&emulator, associate_region, trial, component).await;

        let (status, disassociated) = emulator
            .call_in(
                signing_region,
                "DisassociateTrialComponent",
                &json!({ "TrialName": trial, "TrialComponentName": component }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case}: expected HTTP 200 from DisassociateTrialComponent, got {status} \
             with {disassociated:?}"
        );
        assert_eq!(
            str_field(&disassociated, "TrialArn"),
            format!("arn:aws:sagemaker:{trial_region}:{ACCOUNT}:{TRIAL_KIND}/{trial}"),
            "case {case}: TrialArn must be the trial's stored ARN, got {disassociated:?}"
        );
        assert_eq!(
            str_field(&disassociated, "TrialComponentArn"),
            format!(
                "arn:aws:sagemaker:{component_region}:{ACCOUNT}:{COMPONENT_KIND}/{component}"
            ),
            "case {case}: TrialComponentArn must be the component's stored ARN, got \
             {disassociated:?}"
        );

        // The dropped membership is what a later reader sees: neither the trial nor the
        // experiment it belongs to holds the component any more.
        let in_trial = listed_names(&emulator, &json!({ "TrialName": trial })).await;
        assert!(
            in_trial.is_empty(),
            "case {case}: ListTrialComponents(TrialName) after the disassociate, got \
             {in_trial:?}"
        );
        let in_experiment = listed_names(
            &emulator,
            &json!({ "ExperimentName": "experiment-for-the-trial" }),
        )
        .await;
        assert!(
            in_experiment.is_empty(),
            "case {case}: ListTrialComponents(ExperimentName) after the disassociate, got \
             {in_experiment:?}"
        );
        assert_eq!(
            membership_count(&emulator),
            0,
            "case {case}: the membership must be gone from the store"
        );

        // Only the EDGE goes: both ends are still resources a reader finds.
        let described = emulator
            .ok("DescribeTrialComponent", &json!({ "TrialComponentName": component }))
            .await;
        assert_eq!(
            str_field(&described, "TrialComponentName"),
            component.as_str(),
            "case {case}: the component itself must survive the disassociate, got \
             {described:?}"
        );
        let described_trial = emulator.ok("DescribeTrial", &json!({ "TrialName": trial })).await;
        assert_eq!(
            str_field(&described_trial, "TrialName"),
            trial.as_str(),
            "case {case}: the trial itself must survive the disassociate, got \
             {described_trial:?}"
        );
    }
}

/// RULE: a membership IS the `(trial, component)` pair, so a disassociate drops exactly that
/// one edge — "This doesn't effect other trials the component is associated with", and the
/// trial keeps every other component it holds.
#[tokio::test]
async fn disassociating_drops_only_the_named_pair() {
    let emulator = Emulator::new();
    for experiment in ["exp-left", "exp-right"] {
        seed_experiment(&emulator, "us-east-1", experiment).await;
    }
    for (experiment, trial) in [
        ("exp-left", "trial-left"),
        ("exp-left", "trial-middle"),
        ("exp-right", "trial-right"),
    ] {
        seed_trial_under(&emulator, "us-east-1", experiment, trial).await;
    }
    for component in ["comp-a", "comp-b"] {
        seed_component(&emulator, "us-east-1", component).await;
    }

    // `comp-a` is part of three trials across two experiments; `trial-left` holds two
    // components.
    for (trial, component) in [
        ("trial-left", "comp-a"),
        ("trial-left", "comp-b"),
        ("trial-middle", "comp-a"),
        ("trial-right", "comp-a"),
    ] {
        associate_in(&emulator, "us-east-1", trial, component).await;
    }
    assert_eq!(membership_count(&emulator), 4, "the seeded membership count");

    emulator
        .ok(
            "DisassociateTrialComponent",
            &json!({ "TrialName": "trial-left", "TrialComponentName": "comp-a" }),
        )
        .await;

    // Rows differ along the filter (trial or experiment), whether the named entity was the
    // one the disassociate touched, and how many components it still holds.
    let expectations: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "the-trial-keeps-its-other-component",
            json!({ "TrialName": "trial-left", "SortBy": "Name", "SortOrder": "Ascending" }),
            vec!["comp-b"],
        ),
        (
            "another-trial-of-the-same-experiment-is-untouched",
            json!({ "TrialName": "trial-middle" }),
            vec!["comp-a"],
        ),
        (
            "a-trial-of-another-experiment-is-untouched",
            json!({ "TrialName": "trial-right" }),
            vec!["comp-a"],
        ),
        (
            // `comp-a` is still reached through `trial-middle`, so the experiment still
            // holds it.
            "the-experiment-unions-its-remaining-trials",
            json!({ "ExperimentName": "exp-left", "SortBy": "Name", "SortOrder": "Ascending" }),
            vec!["comp-a", "comp-b"],
        ),
        (
            "the-other-experiment-is-untouched",
            json!({ "ExperimentName": "exp-right" }),
            vec!["comp-a"],
        ),
    ];
    for (case, filter, expected) in &expectations {
        let listed = listed_names(&emulator, filter).await;
        assert_eq!(
            listed,
            expected.iter().map(|name| name.to_string()).collect::<Vec<_>>(),
            "case {case}: ListTrialComponents selection after the disassociate, got \
             {listed:?}"
        );
    }
    assert_eq!(
        membership_count(&emulator),
        3,
        "exactly one membership must have been dropped"
    );
}

/// RULE: an `ExperimentEntityName` "is not case-sensitive", so every spelling of a name
/// denotes the one stored entity: a disassociate written in another case drops the same one
/// membership and answers the ARNs carrying each record's own spelling.
#[tokio::test]
async fn disassociating_folds_the_case_of_both_names() {
    let stored_trial_arn =
        format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:{TRIAL_KIND}/Trial-Mixed-Case");
    let stored_component_arn =
        format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:{COMPONENT_KIND}/Component-Mixed-Case");

    // Rows differ along which end is re-cased and how: lower, upper, and the stored
    // spelling itself. Each row starts from its own emulator, because the membership the
    // row drops is the only one there is.
    let cases: [(&str, &str, &str); 4] = [
        ("as-stored", "Trial-Mixed-Case", "Component-Mixed-Case"),
        ("lowered-trial", "trial-mixed-case", "Component-Mixed-Case"),
        ("uppered-component", "Trial-Mixed-Case", "COMPONENT-MIXED-CASE"),
        ("both-lowered", "trial-mixed-case", "component-mixed-case"),
    ];
    for (case, trial, component) in cases {
        let emulator = Emulator::new();
        seed_trial(&emulator, "us-east-1", "Exp-Mixed", "Trial-Mixed-Case").await;
        seed_component(&emulator, "us-east-1", "Component-Mixed-Case").await;
        associate_in(&emulator, "us-east-1", "Trial-Mixed-Case", "Component-Mixed-Case").await;

        let disassociated = emulator
            .ok(
                "DisassociateTrialComponent",
                &json!({ "TrialName": trial, "TrialComponentName": component }),
            )
            .await;
        assert_eq!(
            str_field(&disassociated, "TrialArn"),
            stored_trial_arn,
            "case {case}: TrialArn must carry the stored spelling, got {disassociated:?}"
        );
        assert_eq!(
            str_field(&disassociated, "TrialComponentArn"),
            stored_component_arn,
            "case {case}: TrialComponentArn must carry the stored spelling, got \
             {disassociated:?}"
        );
        assert_eq!(
            membership_count(&emulator),
            0,
            "case {case}: every spelling names the ONE membership"
        );
        // Whatever case the filter is written in, the trial now holds nothing.
        for spelling in ["Trial-Mixed-Case", "trial-mixed-case", "TRIAL-MIXED-CASE"] {
            let listed = listed_names(&emulator, &json!({ "TrialName": spelling })).await;
            assert!(
                listed.is_empty(),
                "case {case}: ListTrialComponents(TrialName={spelling}), got {listed:?}"
            );
        }
    }
}

/// RULE: the pair is the whole identity of the membership, so a disassociate naming a pair
/// the store does not hold has nothing to drop: it answers both stored ARNs, models no
/// failure for the absent edge, and leaves every recorded membership in place.
#[tokio::test]
async fn disassociating_a_pair_that_is_not_recorded_changes_nothing() {
    let emulator = Emulator::new();
    seed_trial(&emulator, "us-east-1", "exp-idem", "trial-idem").await;
    seed_trial_under(&emulator, "us-east-1", "exp-idem", "trial-other").await;
    seed_component(&emulator, "us-east-1", "component-idem").await;
    seed_component(&emulator, "us-east-1", "component-other").await;
    associate_in(&emulator, "us-east-1", "trial-other", "component-other").await;

    let expected_trial_arn =
        format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:{TRIAL_KIND}/trial-idem");
    let expected_component_arn =
        format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:{COMPONENT_KIND}/component-idem");

    // Rows: a pair never associated at all, the same request repeated (the second call is
    // the repeat of a disassociate that DID drop an edge), and that repeat written in
    // another case.
    let body =
        json!({ "TrialName": "trial-idem", "TrialComponentName": "component-idem" });
    let cases: [(&str, Value); 4] = [
        ("never-associated", body.clone()),
        ("never-associated-other-case", json!({
            "TrialName": "TRIAL-IDEM",
            "TrialComponentName": "Component-Idem",
        })),
        ("after-an-associate-and-a-disassociate", body.clone()),
        ("repeat-in-another-case", json!({
            "TrialName": "Trial-Idem",
            "TrialComponentName": "COMPONENT-IDEM",
        })),
    ];
    for (index, (case, request)) in cases.iter().enumerate() {
        if index == 2 {
            // Record the edge and drop it, so the rows that follow repeat a disassociate
            // that really did remove something.
            associate_in(&emulator, "us-east-1", "trial-idem", "component-idem").await;
            emulator.ok("DisassociateTrialComponent", &body).await;
        }
        let (status, answered) = emulator.call("DisassociateTrialComponent", request).await;
        assert_eq!(
            status, 200,
            "case {case}: expected HTTP 200, got {status} with {answered:?}"
        );
        assert_eq!(
            str_field(&answered, "TrialArn"),
            expected_trial_arn,
            "case {case}: TrialArn, got {answered:?}"
        );
        assert_eq!(
            str_field(&answered, "TrialComponentArn"),
            expected_component_arn,
            "case {case}: TrialComponentArn, got {answered:?}"
        );
        assert_eq!(
            membership_count(&emulator),
            1,
            "case {case}: only the unrelated membership may remain"
        );
        let untouched = listed_names(&emulator, &json!({ "TrialName": "trial-other" })).await;
        assert_eq!(
            untouched,
            vec!["component-other".to_string()],
            "case {case}: the unrelated membership must be untouched, got {untouched:?}"
        );
    }
}

/// RULE: a membership relates two STORED entities, so a disassociate naming an end the
/// account does not hold is refused with the modelled `ResourceNotFound` (HTTP 400) — never
/// a 200, never a 501 — the message names the end it could not resolve by that end's ARN,
/// and no membership is dropped.
#[tokio::test]
async fn disassociating_an_end_that_does_not_exist_is_refused_and_drops_nothing() {
    let emulator = Emulator::new();
    seed_trial(&emulator, "us-east-1", "exp-present", "trial-present").await;
    seed_component(&emulator, "us-east-1", "component-present").await;
    // A second component created in another Region, so a row can show that the stores are
    // keyed by the account-unique name rather than by the signing Region.
    seed_component(&emulator, "eu-west-3", "component-elsewhere").await;
    associate_in(&emulator, "us-east-1", "trial-present", "component-present").await;
    associate_in(&emulator, "us-east-1", "trial-present", "component-elsewhere").await;

    // Rows differ along which end is missing, what the missing name looks like (never
    // created, the OTHER end's name, a name differing from a stored one by more than case),
    // and whether both ends are missing.
    let cases: Vec<(&str, Value, String)> = vec![
        (
            "trial-never-created",
            json!({ "TrialName": "trial-absent", "TrialComponentName": "component-present" }),
            format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:{TRIAL_KIND}/trial-absent"),
        ),
        (
            "component-never-created",
            json!({ "TrialName": "trial-present", "TrialComponentName": "component-absent" }),
            format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:{COMPONENT_KIND}/component-absent"),
        ),
        (
            // Both missing: the trial is resolved first, so the trial is the end reported.
            "both-ends-missing",
            json!({ "TrialName": "nothing-here", "TrialComponentName": "nothing-there" }),
            format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:{TRIAL_KIND}/nothing-here"),
        ),
        (
            // A component is not a trial: naming one where the trial goes resolves no
            // trial.
            "component-name-where-the-trial-goes",
            json!({
                "TrialName": "component-present",
                "TrialComponentName": "component-present",
            }),
            format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:{TRIAL_KIND}/component-present"),
        ),
        (
            // A trial is not a component either.
            "trial-name-where-the-component-goes",
            json!({ "TrialName": "trial-present", "TrialComponentName": "trial-present" }),
            format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:{COMPONENT_KIND}/trial-present"),
        ),
        (
            // A name differing from a stored one by more than case is a different name.
            "near-miss-name",
            json!({ "TrialName": "trial-present", "TrialComponentName": "component-presents" }),
            format!(
                "arn:aws:sagemaker:us-east-1:{ACCOUNT}:{COMPONENT_KIND}/component-presents"
            ),
        ),
    ];

    for (case, body, named) in &cases {
        let (status, refused) = emulator.call("DisassociateTrialComponent", body).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} with {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "ResourceNotFound",
            "case {case}: error code, got {refused:?}"
        );
        let reported = message(&refused);
        assert!(
            reported.contains(named),
            "case {case}: expected the message to name {named}, got {reported:?}"
        );
    }

    // Every refusal above dropped nothing: both seeded memberships are still there.
    assert_eq!(
        membership_count(&emulator),
        2,
        "a refused disassociate must drop no membership"
    );
    let listed = listed_names(
        &emulator,
        &json!({ "TrialName": "trial-present", "SortBy": "Name", "SortOrder": "Ascending" }),
    )
    .await;
    assert_eq!(
        listed,
        vec!["component-elsewhere".to_string(), "component-present".to_string()],
        "the refused disassociates must leave the trial's membership alone, got {listed:?}"
    );

    // The component created in another Region is addressed by name, so its disassociate is
    // served and answers that component's own stored ARN.
    let disassociated = emulator
        .ok(
            "DisassociateTrialComponent",
            &json!({
                "TrialName": "trial-present",
                "TrialComponentName": "component-elsewhere",
            }),
        )
        .await;
    assert_eq!(
        str_field(&disassociated, "TrialComponentArn"),
        format!("arn:aws:sagemaker:eu-west-3:{ACCOUNT}:{COMPONENT_KIND}/component-elsewhere"),
        "case component-in-another-region: TrialComponentArn, got {disassociated:?}"
    );
    let remaining = listed_names(&emulator, &json!({ "TrialName": "trial-present" })).await;
    assert_eq!(
        remaining,
        vec!["component-present".to_string()],
        "only the disassociated membership may be gone, got {remaining:?}"
    );
}
