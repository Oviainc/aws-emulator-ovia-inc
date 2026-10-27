// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for SageMaker's `AssociateTrialComponent`.
//!
//! Every case goes over the wire (`POST /` with
//! `X-Amz-Target: SageMaker.AssociateTrialComponent`, `awsJson1_1`) through the same
//! `MultiServiceRouter` the production router uses, so what these tests exercise is what a
//! real SDK client gets. The harness is the one `inference_deployment_lifecycle` declares,
//! not a second copy of it.
//!
//! The tests assert RULES over a TABLE of inputs rather than one recorded call: the
//! membership the call records, read back through `ListTrialComponents`' `TrialName` and
//! `ExperimentName` filters; the stored ARNs it answers for either end, whatever Region
//! each end was created in and whichever Region the associate signs for; the many-to-many
//! shape of the relation ("a trial component can be associated with multiple trials"); the
//! case folding `ExperimentEntityName` documents; that repeating one association changes
//! nothing; and the modelled `ResourceNotFound` for either end the account does not hold.

use serde_json::{json, Value};
use service_com_amazonaws_sagemaker_sagemaker::ComAmazonawsSagemakerSagemakerState;

use crate::inference_deployment_lifecycle::{error_code, message, str_field, Emulator, ACCOUNT};

/// The ARN resource kind `TrialComponentArn` is modelled with
/// (`^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:experiment-trial-component/`,
/// smithy_query operation=AssociateTrialComponent), spelled out here so the test pins the
/// wire form rather than echoing the constant under test.
const COMPONENT_KIND: &str = "experiment-trial-component";

/// The ARN resource kind `TrialArn` is modelled with
/// (`^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:experiment-trial/`, smithy_query
/// operation=AssociateTrialComponent). See [`COMPONENT_KIND`].
const TRIAL_KIND: &str = "experiment-trial";

/// A name of exactly `length` characters that satisfies `ExperimentEntityName`'s pattern
/// `^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,119}$` (`len 1..120`, smithy_query
/// operation=AssociateTrialComponent).
fn name_of_length(length: usize) -> String {
    "c".repeat(length)
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

/// Create the trial `trial` under the experiment `experiment`, in `region`. The experiment
/// is created too, so one call seeds a trial nothing else shares an experiment with.
async fn seed_trial(emulator: &Emulator, region: &str, experiment: &str, trial: &str) {
    seed_experiment(emulator, region, experiment).await;
    seed_trial_under(emulator, region, experiment, trial).await;
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
        .call_in(
            region,
            "CreateTrialComponent",
            &json!({ "TrialComponentName": component }),
        )
        .await;
    assert_eq!(
        status, 200,
        "seeding component {component}: expected HTTP 200, got {status} with {created:?}"
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

/// How many memberships the shared store holds, so a refusal can be shown to have written
/// nothing at all rather than merely nothing a listing happens to report.
fn membership_count(emulator: &Emulator) -> usize {
    let state = emulator
        .engine
        .state::<ComAmazonawsSagemakerSagemakerState>();
    // The guard is a named local so it is dropped before `state`, the handle it borrows.
    let store = state.lock();
    store.trial_component_trials.len()
}

/// RULE: `AssociateTrialComponent` makes the named component part of the named trial — the
/// membership `ListTrialComponents`' `TrialName` filter reports — and answers the STORED
/// ARN of each end, so each ARN names the Region its own resource was created in rather
/// than the Region this call signed for.
#[tokio::test]
async fn associating_a_component_records_the_membership_and_answers_both_stored_arns() {
    // Rows differ along what the rule generalises over: the name shapes of both ends
    // (single character, digits, hyphens, mixed case, each published length bound), the
    // Region each end was created in, and the Region the associate itself signs for —
    // including rows where all three differ.
    let cases: Vec<(&str, String, String, &str, &str, &str)> = vec![
        (
            "same-region",
            "trial-one".to_string(),
            "component-one".to_string(),
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
        ),
        (
            "name-length-upper-bound",
            "t".repeat(120),
            name_of_length(120),
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
        ),
        (
            "ends-created-in-different-regions",
            "trial-cross-region".to_string(),
            "component-cross-region".to_string(),
            "ca-central-1",
            "eu-west-3",
            "us-east-1",
        ),
    ];

    for (case, trial, component, trial_region, component_region, signing_region) in &cases {
        let emulator = Emulator::new();
        seed_trial(&emulator, trial_region, "experiment-for-the-trial", trial).await;
        seed_component(&emulator, component_region, component).await;

        let (status, associated) = emulator
            .call_in(
                signing_region,
                "AssociateTrialComponent",
                &json!({ "TrialName": trial, "TrialComponentName": component }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case}: expected HTTP 200 from AssociateTrialComponent, got {status} \
             with {associated:?}"
        );
        assert_eq!(
            str_field(&associated, "TrialArn"),
            format!("arn:aws:sagemaker:{trial_region}:{ACCOUNT}:{TRIAL_KIND}/{trial}"),
            "case {case}: TrialArn must be the trial's stored ARN, got {associated:?}"
        );
        assert_eq!(
            str_field(&associated, "TrialComponentArn"),
            format!(
                "arn:aws:sagemaker:{component_region}:{ACCOUNT}:{COMPONENT_KIND}/{component}"
            ),
            "case {case}: TrialComponentArn must be the component's stored ARN, got \
             {associated:?}"
        );

        // The membership is what a later reader sees: the trial now holds the component,
        // and the experiment the trial belongs to holds it too.
        let in_trial = listed_names(&emulator, &json!({ "TrialName": trial })).await;
        assert_eq!(
            in_trial,
            vec![component.clone()],
            "case {case}: ListTrialComponents(TrialName) after the associate, got \
             {in_trial:?}"
        );
        let in_experiment = listed_names(
            &emulator,
            &json!({ "ExperimentName": "experiment-for-the-trial" }),
        )
        .await;
        assert_eq!(
            in_experiment,
            vec![component.clone()],
            "case {case}: ListTrialComponents(ExperimentName) after the associate, got \
             {in_experiment:?}"
        );
    }
}

/// RULE: the relation is many-to-many — "a trial component can be associated with multiple
/// trials" (the operation's model doc) and a trial holds every component associated with
/// it — so each association stands on its own and selects only the pair it named.
#[tokio::test]
async fn a_component_can_be_part_of_several_trials_and_a_trial_of_several_components() {
    let emulator = Emulator::new();
    for experiment in ["exp-left", "exp-right"] {
        seed_experiment(&emulator, "us-east-1", experiment).await;
    }
    for (experiment, trial) in [
        ("exp-left", "trial-left"),
        ("exp-left", "trial-middle"),
        ("exp-right", "trial-right"),
        ("exp-right", "trial-unattached"),
    ] {
        seed_trial_under(&emulator, "us-east-1", experiment, trial).await;
    }
    for component in ["comp-a", "comp-b", "comp-c"] {
        seed_component(&emulator, "us-east-1", component).await;
    }

    // `comp-a` is part of three trials across two experiments, `trial-left` holds two
    // components, `comp-c` is part of none, and `trial-unattached` holds none.
    let pairs = [
        ("trial-left", "comp-a"),
        ("trial-left", "comp-b"),
        ("trial-middle", "comp-a"),
        ("trial-right", "comp-a"),
    ];
    for (trial, component) in pairs {
        emulator
            .ok(
                "AssociateTrialComponent",
                &json!({ "TrialName": trial, "TrialComponentName": component }),
            )
            .await;
    }

    // Rows differ along the filter (trial or experiment), the number of components the
    // named entity holds, and whether a component is reached through one trial or several.
    let expectations: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "trial-of-two-components",
            json!({ "TrialName": "trial-left", "SortBy": "Name", "SortOrder": "Ascending" }),
            vec!["comp-a", "comp-b"],
        ),
        (
            "trial-of-one-component",
            json!({ "TrialName": "trial-middle" }),
            vec!["comp-a"],
        ),
        (
            "trial-holding-nothing",
            json!({ "TrialName": "trial-unattached" }),
            vec![],
        ),
        (
            // One component reached through two trials of the experiment is still one
            // component.
            "experiment-unions-its-trials",
            json!({ "ExperimentName": "exp-left", "SortBy": "Name", "SortOrder": "Ascending" }),
            vec!["comp-a", "comp-b"],
        ),
        (
            "other-experiment",
            json!({ "ExperimentName": "exp-right" }),
            vec!["comp-a"],
        ),
    ];
    for (case, filter, expected) in &expectations {
        let listed = listed_names(&emulator, filter).await;
        assert_eq!(
            listed,
            expected.iter().map(|name| name.to_string()).collect::<Vec<_>>(),
            "case {case}: ListTrialComponents selection, got {listed:?}"
        );
    }
}

/// RULE: an `ExperimentEntityName` "is not case-sensitive", so every spelling of a name
/// denotes the one stored entity: an associate written in another case records the same one
/// membership and answers the ARNs carrying each record's own spelling.
#[tokio::test]
async fn associating_folds_the_case_of_both_names() {
    let emulator = Emulator::new();
    seed_trial(&emulator, "us-east-1", "Exp-Mixed", "Trial-Mixed-Case").await;
    seed_component(&emulator, "us-east-1", "Component-Mixed-Case").await;

    let stored_trial_arn =
        format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:{TRIAL_KIND}/Trial-Mixed-Case");
    let stored_component_arn = format!(
        "arn:aws:sagemaker:us-east-1:{ACCOUNT}:{COMPONENT_KIND}/Component-Mixed-Case"
    );

    // Rows differ along which end is re-cased and how: lower, upper, and the stored
    // spelling itself.
    let cases: [(&str, &str, &str); 4] = [
        ("as-stored", "Trial-Mixed-Case", "Component-Mixed-Case"),
        ("lowered-trial", "trial-mixed-case", "Component-Mixed-Case"),
        ("uppered-component", "Trial-Mixed-Case", "COMPONENT-MIXED-CASE"),
        ("both-lowered", "trial-mixed-case", "component-mixed-case"),
    ];
    for (case, trial, component) in cases {
        let associated = emulator
            .ok(
                "AssociateTrialComponent",
                &json!({ "TrialName": trial, "TrialComponentName": component }),
            )
            .await;
        assert_eq!(
            str_field(&associated, "TrialArn"),
            stored_trial_arn,
            "case {case}: TrialArn must carry the stored spelling, got {associated:?}"
        );
        assert_eq!(
            str_field(&associated, "TrialComponentArn"),
            stored_component_arn,
            "case {case}: TrialComponentArn must carry the stored spelling, got \
             {associated:?}"
        );
        assert_eq!(
            membership_count(&emulator),
            1,
            "case {case}: every spelling names the ONE membership"
        );
        // Whatever case the filter is written in, the one membership is what it reports.
        for spelling in ["Trial-Mixed-Case", "trial-mixed-case", "TRIAL-MIXED-CASE"] {
            let listed = listed_names(&emulator, &json!({ "TrialName": spelling })).await;
            assert_eq!(
                listed,
                vec!["Component-Mixed-Case".to_string()],
                "case {case}: ListTrialComponents(TrialName={spelling}), got {listed:?}"
            );
        }
    }
}

/// RULE: a membership IS the `(trial, component)` pair, so repeating an association that
/// already exists answers the same two ARNs, files no second membership, and leaves the
/// component itself exactly as it was.
#[tokio::test]
async fn repeating_an_association_changes_nothing() {
    let emulator = Emulator::new();
    seed_trial(&emulator, "us-east-1", "exp-repeat", "trial-repeat").await;
    seed_component(&emulator, "us-east-1", "component-repeat").await;

    let body = json!({ "TrialName": "trial-repeat", "TrialComponentName": "component-repeat" });
    let first = emulator.ok("AssociateTrialComponent", &body).await;
    let described_once = emulator
        .ok(
            "DescribeTrialComponent",
            &json!({ "TrialComponentName": "component-repeat" }),
        )
        .await;

    // Rows: the same request repeated, and the same pair written in another case — both
    // name the membership that is already recorded.
    let repeats: [(&str, Value); 2] = [
        ("identical-request", body.clone()),
        (
            "other-case",
            json!({ "TrialName": "TRIAL-REPEAT", "TrialComponentName": "Component-Repeat" }),
        ),
    ];
    for (case, repeat) in repeats {
        let again = emulator.ok("AssociateTrialComponent", &repeat).await;
        assert_eq!(
            again, first,
            "case {case}: a repeated associate must answer identically, got {again:?}"
        );
        assert_eq!(
            membership_count(&emulator),
            1,
            "case {case}: a repeated associate must file no second membership"
        );
        let listed = listed_names(&emulator, &json!({ "TrialName": "trial-repeat" })).await;
        assert_eq!(
            listed,
            vec!["component-repeat".to_string()],
            "case {case}: the trial must hold the component exactly once, got {listed:?}"
        );
        let described_again = emulator
            .ok(
                "DescribeTrialComponent",
                &json!({ "TrialComponentName": "component-repeat" }),
            )
            .await;
        assert_eq!(
            described_again, described_once,
            "case {case}: associating must not alter the component, got {described_again:?}"
        );
    }
}

/// RULE: a membership relates two STORED entities, so an associate naming an end the
/// account does not hold is refused with the modelled `ResourceNotFound` (HTTP 400) —
/// never a 200, never a 501 — the message names the end it could not resolve by that end's
/// ARN, and nothing is recorded.
#[tokio::test]
async fn associating_an_end_that_does_not_exist_is_refused_and_records_nothing() {
    let emulator = Emulator::new();
    seed_trial(&emulator, "us-east-1", "exp-present", "trial-present").await;
    seed_component(&emulator, "us-east-1", "component-present").await;
    // A second component created in another Region, so a row can show that an ARN of the
    // signing Region is NOT what resolves an entity of another one.
    seed_component(&emulator, "eu-west-3", "component-elsewhere").await;

    // Rows differ along which end is missing, what the missing name looks like (never
    // created, the OTHER end's name, a name differing from a stored one by more than
    // case), and whether both ends are missing.
    let cases: Vec<(&str, Value, String)> = vec![
        (
            "trial-never-created",
            json!({ "TrialName": "trial-absent", "TrialComponentName": "component-present" }),
            format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:{TRIAL_KIND}/trial-absent"),
        ),
        (
            "component-never-created",
            json!({ "TrialName": "trial-present", "TrialComponentName": "component-absent" }),
            format!(
                "arn:aws:sagemaker:us-east-1:{ACCOUNT}:{COMPONENT_KIND}/component-absent"
            ),
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
            format!(
                "arn:aws:sagemaker:us-east-1:{ACCOUNT}:{COMPONENT_KIND}/trial-present"
            ),
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
        let (status, refused) = emulator.call("AssociateTrialComponent", body).await;
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

    // The component created in another Region is found by name — the stores are keyed by
    // the account-unique name — so this is a 200 rather than one of the refusals above.
    let associated = emulator
        .ok(
            "AssociateTrialComponent",
            &json!({
                "TrialName": "trial-present",
                "TrialComponentName": "component-elsewhere",
            }),
        )
        .await;
    assert_eq!(
        str_field(&associated, "TrialComponentArn"),
        format!("arn:aws:sagemaker:eu-west-3:{ACCOUNT}:{COMPONENT_KIND}/component-elsewhere"),
        "case component-in-another-region: TrialComponentArn, got {associated:?}"
    );

    // Only the one association that was served is recorded: every refusal above wrote
    // nothing.
    assert_eq!(
        membership_count(&emulator),
        1,
        "a refused associate must record no membership"
    );
    let listed = listed_names(&emulator, &json!({ "TrialName": "trial-present" })).await;
    assert_eq!(
        listed,
        vec!["component-elsewhere".to_string()],
        "the refused associates must leave the trial's membership alone, got {listed:?}"
    );
}
