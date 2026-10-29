// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for SageMaker's `DeleteExperiment`: `CreateExperiment` ->
//! `DescribeExperiment` -> `DeleteExperiment` -> `DescribeExperiment` refused.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.DeleteExperiment`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, so what
//! these tests exercise is what a real SDK client gets. The harness is the one
//! `inference_deployment_lifecycle` declares, not a second copy of it.
//!
//! The tests assert RULES over a TABLE of inputs rather than one recorded call: the delete
//! releases the experiment the name resolves to and answers that record's STORED ARN,
//! whatever Region it was created in and whichever Region the call signs for; the removal is
//! total (a later read, and a second delete, are refused with the modelled
//! `ResourceNotFound`) and frees the name; the case folding `ExperimentEntityName`
//! documents; only the named experiment goes; an experiment trials are still associated
//! with is refused, because "all trials associated with the experiment must be deleted
//! first" (smithy_query operation=DeleteExperiment); and an experiment that is still the
//! source of a lineage association is refused the way every lineage-entity delete refuses
//! one.
//!
//! The not-found refusal is pinned to the sentence this family of resources records — the
//! entity kind, then the identifier RESOLVED to its ARN in single quotes — as the trial
//! suites pin it for the sibling resource addressed the same way.

use serde_json::{json, Value};
use service_com_amazonaws_sagemaker_sagemaker::ComAmazonawsSagemakerSagemakerState;

use crate::inference_deployment_lifecycle::{error_code, message, str_field, Emulator, ACCOUNT};

/// The ARN resource kind `ExperimentArn` is modelled with
/// (`^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:experiment/`, smithy_query
/// operation=DeleteExperiment), spelled out here so the test pins the wire form rather than
/// echoing the constant under test.
const EXPERIMENT_KIND: &str = "experiment";

/// The sentence SageMaker refuses the delete of a lineage entity with a live outgoing
/// association with — fixed text, as the `lineage_entity_association_guard` suite pins it
/// for the other entity kinds an `AssociationEntityArn` names.
const GUARD_MESSAGE: &str = "Cannot delete entity with associations.";

/// A name of exactly `length` characters that satisfies `ExperimentEntityName`'s pattern
/// `^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,119}$` (`len 1..120`, smithy_query
/// operation=DeleteExperiment).
fn name_of_length(length: usize) -> String {
    "e".repeat(length)
}

/// The ARN an experiment created in `region` under `name` is stored with.
fn experiment_arn(region: &str, name: &str) -> String {
    format!("arn:aws:sagemaker:{region}:{ACCOUNT}:{EXPERIMENT_KIND}/{name}")
}

/// Create the experiment `experiment`, signing for `region`.
async fn seed_experiment_in(emulator: &Emulator, region: &str, experiment: &str) {
    let (status, created) = emulator
        .call_in(
            region,
            "CreateExperiment",
            &json!({ "ExperimentName": experiment }),
        )
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

/// Call `target` in `region` and require a 200, returning the parsed output. The shared
/// harness's own `ok` signs for its default Region, and these cases work in several.
async fn ok_in(emulator: &Emulator, region: &str, target: &str, body: &Value) -> Value {
    let (status, parsed) = emulator.call_in(region, target, body).await;
    assert_eq!(
        status, 200,
        "case {target} in {region}: expected HTTP 200, got {status} with {parsed:?}"
    );
    parsed
}

/// How many experiments the shared store holds, so a refused delete can be shown to have
/// removed nothing at all rather than merely nothing a reader happens to report.
fn experiment_count(emulator: &Emulator) -> usize {
    let state = emulator
        .engine
        .state::<ComAmazonawsSagemakerSagemakerState>();
    // The guard is a named local so it is dropped before `state`, the handle it borrows.
    let store = state.lock();
    store.experiments.len()
}

/// How many trials the shared store holds.
fn trial_count(emulator: &Emulator) -> usize {
    let state = emulator
        .engine
        .state::<ComAmazonawsSagemakerSagemakerState>();
    let store = state.lock();
    store.trials.len()
}

/// RULE: `DeleteExperiment` releases the ONE stored experiment its name resolves to — the
/// name is not case-sensitive, so every spelling denotes that one record — and answers the
/// ARN that record carries, so the ARN names the Region the experiment was created in rather
/// than the Region this call signed for. Afterwards no reader finds it, a second delete is
/// refused, and the name is free for another create.
#[tokio::test]
async fn deleting_releases_the_experiment_and_answers_its_stored_arn() {
    // Rows differ along what the rule generalises over: the name shape (single character,
    // each published length bound, digits, hyphens, mixed case), the spelling the delete is
    // written in against the spelling the create stored, the Region the experiment was
    // created in and the Region the delete signs for.
    let cases: Vec<(&str, String, String, &str, &str)> = vec![
        (
            "plain-name",
            "experiment-one".to_string(),
            "experiment-one".to_string(),
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
            "Experiment9000".to_string(),
            "Experiment9000".to_string(),
            "ap-southeast-2",
            "ap-southeast-2",
        ),
        (
            "lowered-spelling",
            "Mixed-Case-Experiment".to_string(),
            "mixed-case-experiment".to_string(),
            "us-east-1",
            "us-east-1",
        ),
        (
            "uppered-spelling",
            "Mixed-Case-Experiment".to_string(),
            "MIXED-CASE-EXPERIMENT".to_string(),
            "ca-central-1",
            "ca-central-1",
        ),
        (
            "created-and-deleted-in-different-regions",
            "experiment-elsewhere".to_string(),
            "experiment-elsewhere".to_string(),
            "eu-west-3",
            "ap-south-1",
        ),
    ];

    for (case, stored, requested, create_region, signing_region) in &cases {
        let emulator = Emulator::new();
        seed_experiment_in(&emulator, create_region, stored).await;
        // A second experiment nothing in this row names, so the row shows that only the
        // experiment the request names is released.
        seed_experiment_in(&emulator, "us-east-1", "bystander-experiment").await;

        let (status, deleted) = emulator
            .call_in(
                signing_region,
                "DeleteExperiment",
                &json!({ "ExperimentName": requested }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case}: expected HTTP 200 from DeleteExperiment, got {status} with \
             {deleted:?}"
        );
        assert_eq!(
            str_field(&deleted, "ExperimentArn"),
            experiment_arn(create_region, stored),
            "case {case}: ExperimentArn must be the experiment's stored ARN, got {deleted:?}"
        );

        // The removal is total: no reader finds it, under any spelling, in any Region.
        for spelling in [stored.as_str(), requested.as_str(), &stored.to_uppercase()] {
            let (read_status, read) = emulator
                .call_in(
                    signing_region,
                    "DescribeExperiment",
                    &json!({ "ExperimentName": spelling }),
                )
                .await;
            assert_eq!(
                read_status, 400,
                "case {case}: DescribeExperiment({spelling}) after the delete: expected \
                 HTTP 400, got {read_status} with {read:?}"
            );
            assert_eq!(
                error_code(&read),
                "ResourceNotFound",
                "case {case}: DescribeExperiment({spelling}) error code, got {read:?}"
            );
        }

        // A second delete has nothing left to release, so it is refused the same way.
        let (again_status, again) = emulator
            .call_in(
                signing_region,
                "DeleteExperiment",
                &json!({ "ExperimentName": requested }),
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

        // Only the named experiment went, and the name it held is free again: the create
        // that would have been refused as a conflict is now served, in the Region it signs
        // for.
        assert_eq!(
            experiment_count(&emulator),
            1,
            "case {case}: only the named experiment may have been released"
        );
        let bystander = emulator
            .ok(
                "DescribeExperiment",
                &json!({ "ExperimentName": "bystander-experiment" }),
            )
            .await;
        assert_eq!(
            str_field(&bystander, "ExperimentName"),
            "bystander-experiment",
            "case {case}: the experiment the delete did not name must survive, got \
             {bystander:?}"
        );
        let (recreate_status, recreated) = emulator
            .call_in(
                signing_region,
                "CreateExperiment",
                &json!({ "ExperimentName": requested }),
            )
            .await;
        assert_eq!(
            recreate_status, 200,
            "case {case}: the deleted name must be free again, got {recreate_status} with \
             {recreated:?}"
        );
        assert_eq!(
            str_field(&recreated, "ExperimentArn"),
            experiment_arn(signing_region, requested),
            "case {case}: the re-created experiment's ARN, got {recreated:?}"
        );
    }
}

/// RULE: a delete addresses a STORED experiment, so one the account does not hold is refused
/// with the modelled `ResourceNotFound` (HTTP 400) — never a 200, never a 501 — the message
/// names the experiment by the ARN the identifier resolves to in the Region the request
/// signed for, and nothing is released.
#[tokio::test]
async fn deleting_an_experiment_that_does_not_exist_is_refused_and_releases_nothing() {
    let emulator = Emulator::new();
    seed_experiment_in(&emulator, "us-east-1", "experiment-present").await;
    // An experiment created in another Region: the store is keyed by the account-unique
    // name, so a delete signing for yet another Region still resolves it — while a
    // never-created name resolves to nothing wherever it is signed for.
    seed_experiment_in(&emulator, "eu-west-3", "experiment-elsewhere").await;
    // A trial and a trial component, so a row can show that a name of ANOTHER resource kind
    // names no experiment.
    seed_trial_in(
        &emulator,
        "us-east-1",
        "experiment-present",
        "trial-present",
    )
    .await;
    let (status, created) = emulator
        .call(
            "CreateTrialComponent",
            &json!({ "TrialComponentName": "component-present" }),
        )
        .await;
    assert_eq!(
        status, 200,
        "seeding component component-present: expected HTTP 200, got {status} with {created:?}"
    );

    // Rows differ along what the missing name looks like (never created, a near miss of a
    // stored one, the name of a DIFFERENT resource kind, each published length bound) and
    // the Region the delete signs for, which is the Region the reported ARN names.
    let longest = name_of_length(120);
    let cases: Vec<(&str, &str, &str)> = vec![
        ("never-created", "us-east-1", "experiment-absent"),
        (
            "near-miss-of-a-stored-name",
            "us-east-1",
            "experiment-presents",
        ),
        ("a-trial-is-not-an-experiment", "us-east-1", "trial-present"),
        (
            "a-component-is-not-an-experiment",
            "us-east-1",
            "component-present",
        ),
        ("another-signing-region", "ap-south-1", "experiment-absent"),
        ("shortest-possible-name", "us-west-2", "x"),
        ("longest-possible-name", "us-west-2", &longest),
    ];

    for (case, region, requested) in &cases {
        let (status, refused) = emulator
            .call_in(
                region,
                "DeleteExperiment",
                &json!({ "ExperimentName": requested }),
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
        let named = experiment_arn(region, requested);
        assert_eq!(
            reported,
            format!("Experiment '{named}' does not exist."),
            "case {case}: the refusal must name the resolved ARN in the template this \
             family of resources records, got {reported:?}"
        );
    }

    // Every refusal above released nothing.
    assert_eq!(
        experiment_count(&emulator),
        2,
        "a refused delete must release no experiment"
    );

    // The experiment created in another Region is addressed by name, so its delete is served
    // and answers that experiment's own stored ARN.
    let deleted = emulator
        .ok(
            "DeleteExperiment",
            &json!({ "ExperimentName": "experiment-elsewhere" }),
        )
        .await;
    assert_eq!(
        str_field(&deleted, "ExperimentArn"),
        experiment_arn("eu-west-3", "experiment-elsewhere"),
        "case experiment-in-another-region: ExperimentArn, got {deleted:?}"
    );
    assert_eq!(
        experiment_count(&emulator),
        1,
        "exactly the named experiment must have been released"
    );
}

/// RULE: "All trials associated with the experiment must be deleted first" (smithy_query
/// operation=DeleteExperiment), so an experiment ANY trial still belongs to is refused as a
/// bad request (HTTP 400 `ValidationException`), keeps every trial and stays an experiment a
/// reader finds; once the last trial is gone the same delete is served.
#[tokio::test]
async fn deleting_an_experiment_its_trials_belong_to_is_refused_until_they_are_gone() {
    // Rows differ along how many trials belong to the experiment and along the spelling the
    // delete is written in — the count the refusal reports is read off the trial store, so
    // it varies with the row.
    let cases: Vec<(&str, &str, &str, Vec<&str>)> = vec![
        (
            "one-trial",
            "experiment-one",
            "experiment-one",
            vec!["trial-left"],
        ),
        (
            "two-trials",
            "experiment-two",
            "experiment-two",
            vec!["trial-left", "trial-middle"],
        ),
        (
            "three-trials",
            "Experiment-Three",
            "Experiment-Three",
            vec!["trial-left", "trial-middle", "trial-right"],
        ),
        (
            // The spelling does not change which experiment is addressed, so the
            // precondition holds under any casing of the name — and a trial that named the
            // experiment in yet another casing still belongs to it.
            "trial-created-under-another-spelling",
            "Experiment-Four",
            "EXPERIMENT-FOUR",
            vec!["trial-right"],
        ),
    ];

    for (case, stored, requested, trials) in &cases {
        let emulator = Emulator::new();
        seed_experiment_in(&emulator, "us-east-1", stored).await;
        for trial in trials {
            seed_trial_in(&emulator, "us-east-1", requested, trial).await;
        }

        let (status, refused) = emulator
            .call(
                "DeleteExperiment",
                &json!({ "ExperimentName": requested }),
            )
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
        let named = experiment_arn("us-east-1", stored);
        assert!(
            reported.contains(&named),
            "case {case}: expected the message to name {named}, got {reported:?}"
        );
        assert!(
            reported.contains(&format!("{} trial(s)", trials.len())),
            "case {case}: expected the message to report {} trial(s), got {reported:?}",
            trials.len()
        );

        // The refusal changed nothing: the experiment and every trial are still there.
        assert_eq!(
            experiment_count(&emulator),
            1,
            "case {case}: a refused delete must release no experiment"
        );
        assert_eq!(
            trial_count(&emulator),
            trials.len(),
            "case {case}: a refused delete must release no trial"
        );
        let described = emulator
            .ok(
                "DescribeExperiment",
                &json!({ "ExperimentName": requested }),
            )
            .await;
        assert_eq!(
            str_field(&described, "ExperimentName"),
            *stored,
            "case {case}: the experiment must survive the refused delete, got {described:?}"
        );

        // Deleting all but the last trial is not enough: the precondition is "all trials",
        // so the delete is still refused while any one of them belongs to the experiment.
        for trial in trials.iter().take(trials.len() - 1) {
            emulator
                .ok("DeleteTrial", &json!({ "TrialName": trial }))
                .await;
            let (still, body) = emulator
                .call(
                    "DeleteExperiment",
                    &json!({ "ExperimentName": requested }),
                )
                .await;
            assert_eq!(
                still, 400,
                "case {case}: still holding trials after deleting {trial}: expected HTTP \
                 400, got {still} with {body:?}"
            );
        }

        // With the last trial gone, the same delete is served.
        let last = trials[trials.len() - 1];
        emulator
            .ok("DeleteTrial", &json!({ "TrialName": last }))
            .await;
        let deleted = emulator
            .ok(
                "DeleteExperiment",
                &json!({ "ExperimentName": requested }),
            )
            .await;
        assert_eq!(
            str_field(&deleted, "ExperimentArn"),
            named,
            "case {case}: the delete after the last trial went, got {deleted:?}"
        );
        assert_eq!(
            experiment_count(&emulator),
            0,
            "case {case}: the experiment must be released once no trial belongs to it"
        );
    }
}

/// RULE: an experiment is one of the lineage entities an `AssociationEntityArn` names, so
/// while any association the account holds starts at it the delete is refused with the fixed
/// HTTP 400 `ValidationException` every lineage-entity delete words that refusal with, and
/// once the edge is released the same delete is served.
#[tokio::test]
async fn an_experiment_sourcing_an_association_is_refused_until_the_edge_is_released() {
    // Rows differ along the Region the entities live in and along the COUNT of edges that
    // start at the experiment: the rule is about any edge, so one left behind still refuses
    // the delete.
    for (row, (region, edge_count)) in [("us-east-1", 1usize), ("eu-west-2", 3usize)]
        .into_iter()
        .enumerate()
    {
        let emulator = Emulator::new();
        let case = format!("{region}/edges={edge_count}");
        let experiment = format!("sourcing-experiment-{row}");
        seed_experiment_in(&emulator, region, &experiment).await;
        let source = experiment_arn(region, &experiment);

        let mut destinations = Vec::new();
        for edge in 0..edge_count {
            let artifact = ok_in(
                &emulator,
                region,
                "CreateArtifact",
                &json!({
                    "ArtifactName": format!("dst{row}x{edge}"),
                    "Source": { "SourceUri": format!("s3://bucket/{row}/{edge}") },
                    "ArtifactType": "DataSet",
                }),
            )
            .await;
            let destination = str_field(&artifact, "ArtifactArn");
            ok_in(
                &emulator,
                region,
                "AddAssociation",
                &json!({ "SourceArn": source, "DestinationArn": destination }),
            )
            .await;
            destinations.push(destination);
        }

        // Released one at a time: the delete stays refused while any edge remains.
        for destination in &destinations {
            let (status, refused) = emulator
                .call_in(
                    region,
                    "DeleteExperiment",
                    &json!({ "ExperimentName": experiment }),
                )
                .await;
            assert_eq!(
                status, 400,
                "case {case}: expected HTTP 400 while an edge starts at the experiment, got \
                 {status} with {refused:?}"
            );
            assert_eq!(
                error_code(&refused),
                "ValidationException",
                "case {case}: error code, got {refused:?}"
            );
            assert_eq!(
                message(&refused),
                GUARD_MESSAGE,
                "case {case}: expected the fixed refusal sentence, got {refused:?}"
            );
            assert_eq!(
                experiment_count(&emulator),
                1,
                "case {case}: a refused delete must release no experiment"
            );
            ok_in(
                &emulator,
                region,
                "DeleteAssociation",
                &json!({ "SourceArn": source, "DestinationArn": destination }),
            )
            .await;
        }

        let (status, deleted) = emulator
            .call_in(
                region,
                "DeleteExperiment",
                &json!({ "ExperimentName": experiment }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case}: with every edge released the delete must succeed, got {status} \
             with {deleted:?}"
        );
        assert_eq!(
            str_field(&deleted, "ExperimentArn"),
            source,
            "case {case}: ExperimentArn, got {deleted:?}"
        );
    }
}
