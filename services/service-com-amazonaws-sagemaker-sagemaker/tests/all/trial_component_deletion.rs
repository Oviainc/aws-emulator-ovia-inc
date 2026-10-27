// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for SageMaker's `DeleteTrialComponent`:
//! `CreateTrialComponent` -> `DescribeTrialComponent` -> `DeleteTrialComponent` ->
//! `DescribeTrialComponent` refused.
//!
//! Every case goes over the wire (`POST /` with
//! `X-Amz-Target: SageMaker.DeleteTrialComponent`, `awsJson1_1`) through the same
//! `MultiServiceRouter` the production router uses, so what these tests exercise is what a
//! real SDK client gets. The harness is the one `inference_deployment_lifecycle` declares,
//! not a second copy of it.
//!
//! The tests assert RULES over a TABLE of inputs rather than one recorded call: the delete
//! releases the component the name resolves to and answers that record's STORED ARN,
//! whatever Region it was created in and whichever Region the call signs for; the removal is
//! total (a later read, and a second delete, are refused with the modelled
//! `ResourceNotFound`) and frees the name; the case folding `ExperimentEntityName`
//! documents; only the named component goes; and a component any trial still holds is
//! refused, because "a trial component must be disassociated from all trials before the
//! trial component can be deleted" (smithy_query operation=DeleteTrialComponent).

use serde_json::json;
use service_com_amazonaws_sagemaker_sagemaker::ComAmazonawsSagemakerSagemakerState;

use crate::inference_deployment_lifecycle::{error_code, message, str_field, Emulator, ACCOUNT};

/// The ARN resource kind `TrialComponentArn` is modelled with
/// (`^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:experiment-trial-component/`,
/// smithy_query operation=DeleteTrialComponent), spelled out here so the test pins the wire
/// form rather than echoing the constant under test.
const COMPONENT_KIND: &str = "experiment-trial-component";

/// A name of exactly `length` characters that satisfies `ExperimentEntityName`'s pattern
/// `^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,119}$` (`len 1..120`, smithy_query
/// operation=DeleteTrialComponent).
fn name_of_length(length: usize) -> String {
    "c".repeat(length)
}

/// The ARN a component created in `region` under `name` is stored with.
fn component_arn(region: &str, name: &str) -> String {
    format!("arn:aws:sagemaker:{region}:{ACCOUNT}:{COMPONENT_KIND}/{name}")
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

/// Create the experiment `experiment` in `us-east-1`.
async fn seed_experiment(emulator: &Emulator, experiment: &str) {
    let (status, created) = emulator
        .call("CreateExperiment", &json!({ "ExperimentName": experiment }))
        .await;
    assert_eq!(
        status, 200,
        "seeding experiment {experiment}: expected HTTP 200, got {status} with {created:?}"
    );
}

/// Create the trial `trial` under `experiment`, creating the experiment too.
async fn seed_trial(emulator: &Emulator, experiment: &str, trial: &str) {
    let (status, created) = emulator
        .call(
            "CreateTrial",
            &json!({ "TrialName": trial, "ExperimentName": experiment }),
        )
        .await;
    assert_eq!(
        status, 200,
        "seeding trial {trial}: expected HTTP 200, got {status} with {created:?}"
    );
}

/// Record the membership of `component` in `trial`.
async fn associate(emulator: &Emulator, trial: &str, component: &str) {
    let (status, associated) = emulator
        .call(
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

/// How many components the shared store holds, so a refused delete can be shown to have
/// removed nothing at all rather than merely nothing a reader happens to report.
fn component_count(emulator: &Emulator) -> usize {
    let state = emulator
        .engine
        .state::<ComAmazonawsSagemakerSagemakerState>();
    // The guard is a named local so it is dropped before `state`, the handle it borrows.
    let store = state.lock();
    store.trial_components.len()
}

/// How many memberships the shared store holds.
fn membership_count(emulator: &Emulator) -> usize {
    let state = emulator
        .engine
        .state::<ComAmazonawsSagemakerSagemakerState>();
    let store = state.lock();
    store.trial_component_trials.len()
}

/// RULE: `DeleteTrialComponent` releases the ONE stored component its name resolves to —
/// the name "is not case-sensitive", so every spelling denotes that one record — and answers
/// the ARN that record carries, so the ARN names the Region the component was created in
/// rather than the Region this call signed for. Afterwards no reader finds it, a second
/// delete is refused, and the name is free for another create.
#[tokio::test]
async fn deleting_releases_the_component_and_answers_its_stored_arn() {
    // Rows differ along what the rule generalises over: the name shape (single character,
    // each published length bound, digits, hyphens, mixed case), the spelling the delete is
    // written in against the spelling the create stored, the Region the component was
    // created in and the Region the delete signs for.
    let cases: Vec<(&str, String, String, &str, &str)> = vec![
        ("plain-name", "component-one".to_string(), "component-one".to_string(), "us-east-1", "us-east-1"),
        (
            "name-length-lower-bound",
            name_of_length(1),
            name_of_length(1),
            "us-west-2",
            "us-west-2",
        ),
        (
            "name-length-upper-bound",
            name_of_length(120),
            name_of_length(120),
            "eu-west-3",
            "eu-west-3",
        ),
        (
            "digits-and-mixed-case",
            "Component9000".to_string(),
            "Component9000".to_string(),
            "ap-southeast-2",
            "ap-southeast-2",
        ),
        (
            "lowered-spelling",
            "Mixed-Case-Component".to_string(),
            "mixed-case-component".to_string(),
            "us-east-1",
            "us-east-1",
        ),
        (
            "uppered-spelling",
            "Mixed-Case-Component".to_string(),
            "MIXED-CASE-COMPONENT".to_string(),
            "ca-central-1",
            "ca-central-1",
        ),
        (
            "created-and-deleted-in-different-regions",
            "component-elsewhere".to_string(),
            "component-elsewhere".to_string(),
            "eu-west-3",
            "ap-south-1",
        ),
    ];

    for (case, stored, requested, create_region, signing_region) in &cases {
        let emulator = Emulator::new();
        seed_component(&emulator, create_region, stored).await;
        // A second component nothing in this row names, so the row shows that only the
        // component the request names is released.
        seed_component(&emulator, "us-east-1", "bystander-component").await;

        let (status, deleted) = emulator
            .call_in(
                signing_region,
                "DeleteTrialComponent",
                &json!({ "TrialComponentName": requested }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case}: expected HTTP 200 from DeleteTrialComponent, got {status} with \
             {deleted:?}"
        );
        assert_eq!(
            str_field(&deleted, "TrialComponentArn"),
            component_arn(create_region, stored),
            "case {case}: TrialComponentArn must be the component's stored ARN, got \
             {deleted:?}"
        );

        // The removal is total: no reader finds it, under any spelling, in any Region.
        for spelling in [stored.as_str(), requested.as_str(), &stored.to_uppercase()] {
            let (read_status, read) = emulator
                .call_in(
                    signing_region,
                    "DescribeTrialComponent",
                    &json!({ "TrialComponentName": spelling }),
                )
                .await;
            assert_eq!(
                read_status, 400,
                "case {case}: DescribeTrialComponent({spelling}) after the delete: expected \
                 HTTP 400, got {read_status} with {read:?}"
            );
            assert_eq!(
                error_code(&read),
                "ResourceNotFound",
                "case {case}: DescribeTrialComponent({spelling}) error code, got {read:?}"
            );
        }

        // A second delete has nothing left to release, so it is refused the same way.
        let (again_status, again) = emulator
            .call_in(
                signing_region,
                "DeleteTrialComponent",
                &json!({ "TrialComponentName": requested }),
            )
            .await;
        assert_eq!(
            again_status, 400,
            "case {case}: a repeated delete: expected HTTP 400, got {again_status} with \
             {again:?}"
        );
        assert_eq!(
            error_code(&again),
            "ResourceNotFound",
            "case {case}: a repeated delete's error code, got {again:?}"
        );

        // Only the named component went, and the name it held is free again: the create
        // that would have been refused as a conflict is now served, in the Region it signs
        // for.
        assert_eq!(
            component_count(&emulator),
            1,
            "case {case}: only the named component may have been released"
        );
        let bystander = emulator
            .ok(
                "DescribeTrialComponent",
                &json!({ "TrialComponentName": "bystander-component" }),
            )
            .await;
        assert_eq!(
            str_field(&bystander, "TrialComponentName"),
            "bystander-component",
            "case {case}: the component the delete did not name must survive, got \
             {bystander:?}"
        );
        let (recreate_status, recreated) = emulator
            .call_in(
                signing_region,
                "CreateTrialComponent",
                &json!({ "TrialComponentName": requested }),
            )
            .await;
        assert_eq!(
            recreate_status, 200,
            "case {case}: the deleted name must be free again, got {recreate_status} with \
             {recreated:?}"
        );
        assert_eq!(
            str_field(&recreated, "TrialComponentArn"),
            component_arn(signing_region, requested),
            "case {case}: the re-created component's ARN, got {recreated:?}"
        );
    }
}

/// RULE: a delete addresses a STORED component, so one the account does not hold is refused
/// with the modelled `ResourceNotFound` (HTTP 400) — never a 200, never a 501 — the message
/// names the component by the ARN the identifier resolves to in the Region the request
/// signed for, and nothing is released.
#[tokio::test]
async fn deleting_a_component_that_does_not_exist_is_refused_and_releases_nothing() {
    let emulator = Emulator::new();
    seed_component(&emulator, "us-east-1", "component-present").await;
    // A component created in another Region: the store is keyed by the account-unique
    // name, so a delete signing for yet another Region still resolves it — while a
    // never-created name resolves to nothing wherever it is signed for.
    seed_component(&emulator, "eu-west-3", "component-elsewhere").await;
    seed_experiment(&emulator, "experiment-present").await;
    seed_trial(&emulator, "experiment-present", "trial-present").await;

    // Rows differ along what the missing name looks like (never created, a near miss of a
    // stored one, the name of a DIFFERENT resource kind, each published length bound) and
    // the Region the delete signs for, which is the Region the reported ARN names.
    let cases: Vec<(&str, &str, &str)> = vec![
        ("never-created", "us-east-1", "component-absent"),
        ("near-miss-of-a-stored-name", "us-east-1", "component-presents"),
        ("a-trial-is-not-a-component", "us-east-1", "trial-present"),
        ("an-experiment-is-not-a-component", "us-east-1", "experiment-present"),
        ("another-signing-region", "ap-south-1", "component-absent"),
        ("shortest-possible-name", "us-west-2", "x"),
    ];

    for (case, region, requested) in &cases {
        let (status, refused) = emulator
            .call_in(
                region,
                "DeleteTrialComponent",
                &json!({ "TrialComponentName": requested }),
            )
            .await;
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
        let named = component_arn(region, requested);
        assert!(
            reported.contains(&named),
            "case {case}: expected the message to name {named}, got {reported:?}"
        );
    }

    // Every refusal above released nothing.
    assert_eq!(
        component_count(&emulator),
        2,
        "a refused delete must release no component"
    );

    // The component created in another Region is addressed by name, so its delete is served
    // and answers that component's own stored ARN.
    let deleted = emulator
        .ok(
            "DeleteTrialComponent",
            &json!({ "TrialComponentName": "component-elsewhere" }),
        )
        .await;
    assert_eq!(
        str_field(&deleted, "TrialComponentArn"),
        component_arn("eu-west-3", "component-elsewhere"),
        "case component-in-another-region: TrialComponentArn, got {deleted:?}"
    );
    assert_eq!(
        component_count(&emulator),
        1,
        "exactly the named component must have been released"
    );
}

/// RULE: "A trial component must be disassociated from all trials before the trial component
/// can be deleted" (smithy_query operation=DeleteTrialComponent), so a component ANY trial
/// still holds is refused as a bad request (HTTP 400 `ValidationException`), keeps every
/// membership and stays a component a reader finds; once the last membership is dropped the
/// same delete is served.
#[tokio::test]
async fn deleting_a_component_a_trial_still_holds_is_refused_until_it_is_disassociated() {
    // Rows differ along how many trials hold the component and across how many experiments,
    // and along the spelling the delete is written in — the count the refusal reports is read
    // off the membership index, so it varies with the row.
    let cases: Vec<(&str, &str, &str, Vec<&str>)> = vec![
        ("one-trial", "comp-one", "comp-one", vec!["trial-left"]),
        (
            "two-trials-of-one-experiment",
            "comp-two",
            "comp-two",
            vec!["trial-left", "trial-middle"],
        ),
        (
            "three-trials-across-two-experiments",
            "Comp-Three",
            "Comp-Three",
            vec!["trial-left", "trial-middle", "trial-right"],
        ),
        (
            // The spelling does not change which component is addressed, so the
            // precondition holds under any casing of the name.
            "membership-held-under-another-spelling",
            "Comp-Four",
            "COMP-FOUR",
            vec!["trial-right"],
        ),
    ];

    for (case, stored, requested, trials) in &cases {
        let emulator = Emulator::new();
        seed_experiment(&emulator, "exp-left").await;
        seed_experiment(&emulator, "exp-right").await;
        seed_trial(&emulator, "exp-left", "trial-left").await;
        seed_trial(&emulator, "exp-left", "trial-middle").await;
        seed_trial(&emulator, "exp-right", "trial-right").await;
        seed_component(&emulator, "us-east-1", stored).await;
        for trial in trials {
            associate(&emulator, trial, requested).await;
        }

        let (status, refused) = emulator
            .call("DeleteTrialComponent", &json!({ "TrialComponentName": requested }))
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} with {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case}: error code, got {refused:?}"
        );
        let reported = message(&refused);
        let named = component_arn("us-east-1", stored);
        assert!(
            reported.contains(&named),
            "case {case}: expected the message to name {named}, got {reported:?}"
        );
        assert!(
            reported.contains(&format!("{} trial(s)", trials.len())),
            "case {case}: expected the message to report {} holding trial(s), got \
             {reported:?}",
            trials.len()
        );

        // The refusal changed nothing: the component and every membership are still there.
        assert_eq!(
            component_count(&emulator),
            1,
            "case {case}: a refused delete must release no component"
        );
        assert_eq!(
            membership_count(&emulator),
            trials.len(),
            "case {case}: a refused delete must drop no membership"
        );
        let described = emulator
            .ok("DescribeTrialComponent", &json!({ "TrialComponentName": requested }))
            .await;
        assert_eq!(
            str_field(&described, "TrialComponentName"),
            *stored,
            "case {case}: the component must survive the refused delete, got {described:?}"
        );

        // Dropping all but the last membership is not enough: the precondition is "all
        // trials", so the delete is still refused while any one of them holds it.
        for trial in trials.iter().take(trials.len() - 1) {
            emulator
                .ok(
                    "DisassociateTrialComponent",
                    &json!({ "TrialName": trial, "TrialComponentName": requested }),
                )
                .await;
            let (still, body) = emulator
                .call("DeleteTrialComponent", &json!({ "TrialComponentName": requested }))
                .await;
            assert_eq!(
                still, 400,
                "case {case}: still held after disassociating {trial}: expected HTTP 400, \
                 got {still} with {body:?}"
            );
        }

        // Disassociated from the last trial, the same delete is served.
        let last = trials[trials.len() - 1];
        emulator
            .ok(
                "DisassociateTrialComponent",
                &json!({ "TrialName": last, "TrialComponentName": requested }),
            )
            .await;
        let deleted = emulator
            .ok("DeleteTrialComponent", &json!({ "TrialComponentName": requested }))
            .await;
        assert_eq!(
            str_field(&deleted, "TrialComponentArn"),
            named,
            "case {case}: the delete after the last disassociate, got {deleted:?}"
        );
        assert_eq!(
            component_count(&emulator),
            0,
            "case {case}: the component must be released once no trial holds it"
        );
        // Only the component goes: the trials it was part of are still there.
        for trial in trials {
            let trial_read = emulator.ok("DescribeTrial", &json!({ "TrialName": trial })).await;
            assert_eq!(
                str_field(&trial_read, "TrialName"),
                *trial,
                "case {case}: trial {trial} must survive the component's delete, got \
                 {trial_read:?}"
            );
        }
    }
}
