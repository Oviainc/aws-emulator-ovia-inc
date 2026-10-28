// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for SageMaker's `DeleteTrial`: `CreateTrial` -> `DescribeTrial`
//! -> `DeleteTrial` -> `DescribeTrial` refused.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.DeleteTrial`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, so what
//! these tests exercise is what a real SDK client gets. The harness is the one
//! `inference_deployment_lifecycle` declares, not a second copy of it.
//!
//! The tests assert RULES over a TABLE of inputs rather than one recorded call: the delete
//! releases the trial the name resolves to and answers that record's STORED ARN, whatever
//! Region it was created in and whichever Region the call signs for; the removal is total
//! (a later read, and a second delete, are refused with the modelled `ResourceNotFound`)
//! and frees the name; the case folding `ExperimentEntityName` documents; only the named
//! trial goes, and its experiment stays; and a trial that trial components still make up is
//! refused, because "all trial components that make up the trial must be deleted first"
//! (smithy_query operation=DeleteTrial).

use serde_json::json;
use service_com_amazonaws_sagemaker_sagemaker::ComAmazonawsSagemakerSagemakerState;

use crate::inference_deployment_lifecycle::{error_code, message, str_field, Emulator, ACCOUNT};

/// The ARN resource kind `TrialArn` is modelled with
/// (`^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:experiment-trial/`, smithy_query
/// operation=DeleteTrial), spelled out here so the test pins the wire form rather than
/// echoing the constant under test.
const TRIAL_KIND: &str = "experiment-trial";

/// A name of exactly `length` characters that satisfies `ExperimentEntityName`'s pattern
/// `^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,119}$` (`len 1..120`, smithy_query
/// operation=DeleteTrial).
fn name_of_length(length: usize) -> String {
    "t".repeat(length)
}

/// The ARN a trial created in `region` under `name` is stored with.
fn trial_arn(region: &str, name: &str) -> String {
    format!("arn:aws:sagemaker:{region}:{ACCOUNT}:{TRIAL_KIND}/{name}")
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

/// Create the trial `trial` under `experiment`, signing for `region`.
async fn seed_trial_in(emulator: &Emulator, region: &str, experiment: &str, trial: &str) {
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

/// Create the trial component `component` in `us-east-1`.
async fn seed_component(emulator: &Emulator, component: &str) {
    let (status, created) = emulator
        .call(
            "CreateTrialComponent",
            &json!({ "TrialComponentName": component }),
        )
        .await;
    assert_eq!(
        status, 200,
        "seeding component {component}: expected HTTP 200, got {status} with {created:?}"
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

/// How many trials the shared store holds, so a refused delete can be shown to have
/// removed nothing at all rather than merely nothing a reader happens to report.
fn trial_count(emulator: &Emulator) -> usize {
    let state = emulator
        .engine
        .state::<ComAmazonawsSagemakerSagemakerState>();
    // The guard is a named local so it is dropped before `state`, the handle it borrows.
    let store = state.lock();
    store.trials.len()
}

/// How many memberships the shared store holds.
fn membership_count(emulator: &Emulator) -> usize {
    let state = emulator
        .engine
        .state::<ComAmazonawsSagemakerSagemakerState>();
    let store = state.lock();
    store.trial_component_trials.len()
}

/// RULE: `DeleteTrial` releases the ONE stored trial its name resolves to — the name is not
/// case-sensitive, so every spelling denotes that one record — and answers the ARN that
/// record carries, so the ARN names the Region the trial was created in rather than the
/// Region this call signed for. Afterwards no reader finds it, a second delete is refused,
/// and the name is free for another create.
#[tokio::test]
async fn deleting_releases_the_trial_and_answers_its_stored_arn() {
    // Rows differ along what the rule generalises over: the name shape (single character,
    // each published length bound, digits, hyphens, mixed case), the spelling the delete is
    // written in against the spelling the create stored, the Region the trial was created
    // in and the Region the delete signs for.
    let cases: Vec<(&str, String, String, &str, &str)> = vec![
        (
            "plain-name",
            "trial-one".to_string(),
            "trial-one".to_string(),
            "us-east-1",
            "us-east-1",
        ),
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
            "Trial9000".to_string(),
            "Trial9000".to_string(),
            "ap-southeast-2",
            "ap-southeast-2",
        ),
        (
            "lowered-spelling",
            "Mixed-Case-Trial".to_string(),
            "mixed-case-trial".to_string(),
            "us-east-1",
            "us-east-1",
        ),
        (
            "uppered-spelling",
            "Mixed-Case-Trial".to_string(),
            "MIXED-CASE-TRIAL".to_string(),
            "ca-central-1",
            "ca-central-1",
        ),
        (
            "created-and-deleted-in-different-regions",
            "trial-elsewhere".to_string(),
            "trial-elsewhere".to_string(),
            "eu-west-3",
            "ap-south-1",
        ),
    ];

    for (case, stored, requested, create_region, signing_region) in &cases {
        let emulator = Emulator::new();
        seed_experiment(&emulator, "experiment-under-test").await;
        seed_trial_in(&emulator, create_region, "experiment-under-test", stored).await;
        // A second trial nothing in this row names, so the row shows that only the trial
        // the request names is released.
        seed_trial_in(&emulator, "us-east-1", "experiment-under-test", "bystander-trial").await;

        let (status, deleted) = emulator
            .call_in(
                signing_region,
                "DeleteTrial",
                &json!({ "TrialName": requested }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case}: expected HTTP 200 from DeleteTrial, got {status} with {deleted:?}"
        );
        assert_eq!(
            str_field(&deleted, "TrialArn"),
            trial_arn(create_region, stored),
            "case {case}: TrialArn must be the trial's stored ARN, got {deleted:?}"
        );

        // The removal is total: no reader finds it, under any spelling, in any Region.
        for spelling in [stored.as_str(), requested.as_str(), &stored.to_uppercase()] {
            let (read_status, read) = emulator
                .call_in(
                    signing_region,
                    "DescribeTrial",
                    &json!({ "TrialName": spelling }),
                )
                .await;
            assert_eq!(
                read_status, 400,
                "case {case}: DescribeTrial({spelling}) after the delete: expected HTTP \
                 400, got {read_status} with {read:?}"
            );
            assert_eq!(
                error_code(&read),
                "ResourceNotFound",
                "case {case}: DescribeTrial({spelling}) error code, got {read:?}"
            );
        }

        // A second delete has nothing left to release, so it is refused the same way.
        let (again_status, again) = emulator
            .call_in(
                signing_region,
                "DeleteTrial",
                &json!({ "TrialName": requested }),
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

        // Only the named trial went, and the name it held is free again: the create that
        // would have been refused as a conflict is now served, in the Region it signs for.
        assert_eq!(
            trial_count(&emulator),
            1,
            "case {case}: only the named trial may have been released"
        );
        let bystander = emulator
            .ok("DescribeTrial", &json!({ "TrialName": "bystander-trial" }))
            .await;
        assert_eq!(
            str_field(&bystander, "TrialName"),
            "bystander-trial",
            "case {case}: the trial the delete did not name must survive, got {bystander:?}"
        );
        // The trial's PARENT is not touched: a delete of the child releases the child only.
        let experiment = emulator
            .ok(
                "DescribeExperiment",
                &json!({ "ExperimentName": "experiment-under-test" }),
            )
            .await;
        assert_eq!(
            str_field(&experiment, "ExperimentName"),
            "experiment-under-test",
            "case {case}: the trial's experiment must survive, got {experiment:?}"
        );
        let (recreate_status, recreated) = emulator
            .call_in(
                signing_region,
                "CreateTrial",
                &json!({
                    "TrialName": requested,
                    "ExperimentName": "experiment-under-test",
                }),
            )
            .await;
        assert_eq!(
            recreate_status, 200,
            "case {case}: the deleted name must be free again, got {recreate_status} with \
             {recreated:?}"
        );
        assert_eq!(
            str_field(&recreated, "TrialArn"),
            trial_arn(signing_region, requested),
            "case {case}: the re-created trial's ARN, got {recreated:?}"
        );
    }
}

/// RULE: a delete addresses a STORED trial, so one the account does not hold is refused with
/// the modelled `ResourceNotFound` (HTTP 400) — never a 200, never a 501 — the message names
/// the trial by the ARN the identifier resolves to in the Region the request signed for, and
/// nothing is released.
#[tokio::test]
async fn deleting_a_trial_that_does_not_exist_is_refused_and_releases_nothing() {
    let emulator = Emulator::new();
    seed_experiment(&emulator, "experiment-present").await;
    seed_trial_in(&emulator, "us-east-1", "experiment-present", "trial-present").await;
    // A trial created in another Region: the store is keyed by the account-unique name, so
    // a delete signing for yet another Region still resolves it — while a never-created
    // name resolves to nothing wherever it is signed for.
    seed_trial_in(&emulator, "eu-west-3", "experiment-present", "trial-elsewhere").await;
    seed_component(&emulator, "component-present").await;

    // Rows differ along what the missing name looks like (never created, a near miss of a
    // stored one, the name of a DIFFERENT resource kind, each published length bound) and
    // the Region the delete signs for, which is the Region the reported ARN names.
    let longest = name_of_length(120);
    let cases: Vec<(&str, &str, &str)> = vec![
        ("never-created", "us-east-1", "trial-absent"),
        ("near-miss-of-a-stored-name", "us-east-1", "trial-presents"),
        ("a-component-is-not-a-trial", "us-east-1", "component-present"),
        ("an-experiment-is-not-a-trial", "us-east-1", "experiment-present"),
        ("another-signing-region", "ap-south-1", "trial-absent"),
        ("shortest-possible-name", "us-west-2", "x"),
        ("longest-possible-name", "us-west-2", &longest),
    ];

    for (case, region, requested) in &cases {
        let (status, refused) = emulator
            .call_in(region, "DeleteTrial", &json!({ "TrialName": requested }))
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
        let named = trial_arn(region, requested);
        assert!(
            reported.contains(&named),
            "case {case}: expected the message to name {named}, got {reported:?}"
        );
    }

    // Every refusal above released nothing.
    assert_eq!(
        trial_count(&emulator),
        2,
        "a refused delete must release no trial"
    );

    // The trial created in another Region is addressed by name, so its delete is served and
    // answers that trial's own stored ARN.
    let deleted = emulator
        .ok("DeleteTrial", &json!({ "TrialName": "trial-elsewhere" }))
        .await;
    assert_eq!(
        str_field(&deleted, "TrialArn"),
        trial_arn("eu-west-3", "trial-elsewhere"),
        "case trial-in-another-region: TrialArn, got {deleted:?}"
    );
    assert_eq!(
        trial_count(&emulator),
        1,
        "exactly the named trial must have been released"
    );
}

/// RULE: "All trial components that make up the trial must be deleted first" (smithy_query
/// operation=DeleteTrial), so a trial ANY component is still part of is refused as a bad
/// request (HTTP 400 `ValidationException`), keeps every membership and stays a trial a
/// reader finds; once the last component is gone the same delete is served.
#[tokio::test]
async fn deleting_a_trial_its_components_still_make_up_is_refused_until_they_are_gone() {
    // Rows differ along how many components make up the trial and along the spelling the
    // delete is written in — the count the refusal reports is read off the membership
    // index, so it varies with the row.
    let cases: Vec<(&str, &str, &str, Vec<&str>)> = vec![
        ("one-component", "trial-one", "trial-one", vec!["comp-left"]),
        (
            "two-components",
            "trial-two",
            "trial-two",
            vec!["comp-left", "comp-middle"],
        ),
        (
            "three-components",
            "Trial-Three",
            "Trial-Three",
            vec!["comp-left", "comp-middle", "comp-right"],
        ),
        (
            // The spelling does not change which trial is addressed, so the precondition
            // holds under any casing of the name.
            "membership-held-under-another-spelling",
            "Trial-Four",
            "TRIAL-FOUR",
            vec!["comp-right"],
        ),
    ];

    for (case, stored, requested, components) in &cases {
        let emulator = Emulator::new();
        seed_experiment(&emulator, "exp-under-test").await;
        seed_trial_in(&emulator, "us-east-1", "exp-under-test", stored).await;
        for component in components {
            seed_component(&emulator, component).await;
            associate(&emulator, requested, component).await;
        }

        let (status, refused) = emulator
            .call("DeleteTrial", &json!({ "TrialName": requested }))
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
        let named = trial_arn("us-east-1", stored);
        assert!(
            reported.contains(&named),
            "case {case}: expected the message to name {named}, got {reported:?}"
        );
        assert!(
            reported.contains(&format!("{} trial component(s)", components.len())),
            "case {case}: expected the message to report {} component(s), got {reported:?}",
            components.len()
        );

        // The refusal changed nothing: the trial and every membership are still there.
        assert_eq!(
            trial_count(&emulator),
            1,
            "case {case}: a refused delete must release no trial"
        );
        assert_eq!(
            membership_count(&emulator),
            components.len(),
            "case {case}: a refused delete must drop no membership"
        );
        let described = emulator
            .ok("DescribeTrial", &json!({ "TrialName": requested }))
            .await;
        assert_eq!(
            str_field(&described, "TrialName"),
            *stored,
            "case {case}: the trial must survive the refused delete, got {described:?}"
        );

        // Removing all but the last component is not enough: the precondition is "all trial
        // components", so the delete is still refused while any one of them makes it up.
        for component in components.iter().take(components.len() - 1) {
            emulator
                .ok(
                    "DisassociateTrialComponent",
                    &json!({ "TrialName": requested, "TrialComponentName": component }),
                )
                .await;
            emulator
                .ok(
                    "DeleteTrialComponent",
                    &json!({ "TrialComponentName": component }),
                )
                .await;
            let (still, body) = emulator
                .call("DeleteTrial", &json!({ "TrialName": requested }))
                .await;
            assert_eq!(
                still, 400,
                "case {case}: still made up after deleting {component}: expected HTTP 400, \
                 got {still} with {body:?}"
            );
        }

        // With the last component gone, the same delete is served.
        let last = components[components.len() - 1];
        emulator
            .ok(
                "DisassociateTrialComponent",
                &json!({ "TrialName": requested, "TrialComponentName": last }),
            )
            .await;
        let deleted = emulator
            .ok("DeleteTrial", &json!({ "TrialName": requested }))
            .await;
        assert_eq!(
            str_field(&deleted, "TrialArn"),
            named,
            "case {case}: the delete after the last component went, got {deleted:?}"
        );
        assert_eq!(
            trial_count(&emulator),
            0,
            "case {case}: the trial must be released once no component makes it up"
        );
        // Only the trial goes: the component disassociated last is still a component a
        // reader finds, because this delete releases the trial alone.
        let survivor = emulator
            .ok(
                "DescribeTrialComponent",
                &json!({ "TrialComponentName": last }),
            )
            .await;
        assert_eq!(
            str_field(&survivor, "TrialComponentName"),
            last,
            "case {case}: the component disassociated last must survive, got {survivor:?}"
        );
    }
}
