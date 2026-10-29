// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for how SageMaker WORDS the refusal of an experiment the account
//! does not hold.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, via the
//! harness `inference_deployment_lifecycle` declares, so what these tests exercise is what
//! a real SDK client gets.
//!
//! The rule asserted here is ONE sentence over a TABLE of inputs rather than one recorded
//! call: every operation that resolves an experiment reports one the account does not hold
//! as `Experiment '<resolved ARN>' does not exist.` — the requested `ExperimentName`
//! resolved to the experiment ARN of the Region the request signed for, SINGLE QUOTED —
//! with the modelled `ResourceNotFound` and HTTP 400. The quotes are what delimit the ARN,
//! so a consumer can cut the identifier back out of the refusal, exactly as the sibling
//! trial suite pins it for the resource addressed the same way.

use serde_json::{json, Value};

use crate::inference_deployment_lifecycle::{error_code, message, str_field, Emulator, ACCOUNT};

/// The ARN resource kind `ExperimentArn` is modelled with
/// (`^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:experiment/`, smithy_query
/// operation=DescribeExperiment), spelled out here so the test pins the wire form rather
/// than echoing the constant under test.
const EXPERIMENT_KIND: &str = "experiment";

/// The experiment ARN an `ExperimentName` resolves to in `region`, as this service mints
/// one.
fn experiment_arn(region: &str, name: &str) -> String {
    format!("arn:aws:sagemaker:{region}:{ACCOUNT}:{EXPERIMENT_KIND}/{name}")
}

/// The whole sentence the refusal must carry for `name` in `region`.
fn expected_message(region: &str, name: &str) -> String {
    format!(
        "Experiment '{}' does not exist.",
        experiment_arn(region, name)
    )
}

/// Every operation that resolves an experiment by name, with the request body each takes.
///
/// `CreateTrial` resolves the parent experiment BEFORE anything else it could refuse, so
/// the experiment is the end it reports; `trial` is a name no trial holds, so the parent is
/// the only thing missing in these rows.
fn experiment_readers(experiment: &str, trial: &str) -> Vec<(&'static str, Value)> {
    vec![
        (
            "DescribeExperiment",
            json!({ "ExperimentName": experiment }),
        ),
        ("DeleteExperiment", json!({ "ExperimentName": experiment })),
        (
            "CreateTrial",
            json!({ "TrialName": trial, "ExperimentName": experiment }),
        ),
    ]
}

/// An emulator holding one experiment (`stored-experiment`), its trial (`stored-trial`) and
/// one trial component (`stored-component`), all created in `region`.
async fn seeded(region: &str) -> Emulator {
    let emulator = Emulator::new();
    for (what, target, body) in [
        (
            "the experiment",
            "CreateExperiment",
            json!({ "ExperimentName": "stored-experiment" }),
        ),
        (
            "the trial",
            "CreateTrial",
            json!({ "TrialName": "stored-trial", "ExperimentName": "stored-experiment" }),
        ),
        (
            "the component",
            "CreateTrialComponent",
            json!({ "TrialComponentName": "stored-component" }),
        ),
    ] {
        let (status, created) = emulator.call_in(region, target, &body).await;
        assert_eq!(
            status, 200,
            "seeding {what}: expected HTTP 200, got {status} with {created:?}"
        );
    }
    emulator
}

/// The identifier a consumer cuts out of `reported` by taking what the single quotes
/// delimit, or `None` when the sentence carries no such pair.
fn quoted_identifier(reported: &str) -> Option<&str> {
    let (_, after) = reported.split_once('\'')?;
    let (inside, _) = after.rsplit_once('\'')?;
    Some(inside)
}

/// RULE: every operation that resolves an experiment reports one the account does not hold
/// as `Experiment '<resolved ARN>' does not exist.` — the requested `ExperimentName`
/// resolved to the `experiment` ARN of the Region the request signed for, single quoted —
/// with the modelled `ResourceNotFound` and HTTP 400.
#[tokio::test]
async fn every_reader_of_a_missing_experiment_reports_the_quoted_resolved_arn() {
    // Rows differ along the signed Region, the shape of the requested name (the published
    // `ExperimentEntityName` bounds `len 1..120` and its pattern
    // `^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,119}$`, smithy_query operation=DescribeExperiment: one
    // character, 120 characters, a digit-leading name, a mixed-case name, hyphen-free) and
    // why the experiment is missing (never created, a near miss of the stored one, the name
    // of a stored resource of ANOTHER kind).
    let rows: [(&str, &str, String); 9] = [
        ("never created", "us-east-1", "experiment-absent".to_string()),
        ("another region", "eu-west-1", "experiment-absent".to_string()),
        ("a third region", "ap-south-1", "experiment-absent".to_string()),
        ("the shortest name", "us-west-2", "A".to_string()),
        ("the longest name", "us-east-1", "n".repeat(120)),
        ("a digit-leading name", "eu-central-1", "9thexperiment".to_string()),
        ("a mixed-case name", "us-east-1", "Absent-Experiment-9".to_string()),
        (
            "a near miss of the stored experiment",
            "us-east-1",
            "stored-experiments".to_string(),
        ),
        // A trial is not an experiment, so its name resolves no experiment.
        (
            "the name of a stored trial",
            "us-east-1",
            "stored-trial".to_string(),
        ),
    ];

    for (case, region, name) in rows {
        // One emulator per row: every row below is a REFUSAL, and a refusal writes nothing,
        // so the readers of the row share the same seeded store.
        let emulator = seeded(region).await;
        for (operation, body) in experiment_readers(&name, "new-trial") {
            let (status, refused) = emulator.call_in(region, operation, &body).await;
            assert_eq!(
                (status, error_code(&refused).as_str()),
                (400, "ResourceNotFound"),
                "case {case:?}/{operation}: expected 400 ResourceNotFound, got {status} \
                 with {refused:?}"
            );
            let reported = message(&refused);
            assert_eq!(
                reported,
                expected_message(region, &name),
                "case {case:?}/{operation}: expected the resolved ARN single-quoted, got \
                 {reported:?}"
            );
            assert_eq!(
                quoted_identifier(&reported),
                Some(experiment_arn(region, &name).as_str()),
                "case {case:?}/{operation}: the quotes must delimit the ARN, got \
                 {reported:?}"
            );
        }
    }
}

/// RULE: the same sentence states an experiment that was deleted — a released name is as
/// missing as one nobody created.
#[tokio::test]
async fn a_deleted_experiment_is_reported_the_same_way_as_one_never_created() {
    for (case, region) in [
        ("after a delete", "us-east-1"),
        ("in another region", "eu-west-1"),
    ] {
        let emulator = seeded(region).await;
        // The experiment's only trial goes first: the delete is refused while any trial is
        // associated with it.
        let (status, released) = emulator
            .call_in(region, "DeleteTrial", &json!({ "TrialName": "stored-trial" }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteTrial, got {status} with \
             {released:?}"
        );
        let (status, deleted) = emulator
            .call_in(
                region,
                "DeleteExperiment",
                &json!({ "ExperimentName": "stored-experiment" }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteExperiment, got {status} with \
             {deleted:?}"
        );

        for (operation, body) in experiment_readers("stored-experiment", "new-trial") {
            let (status, refused) = emulator.call_in(region, operation, &body).await;
            assert_eq!(
                (status, error_code(&refused).as_str()),
                (400, "ResourceNotFound"),
                "case {case:?}/{operation}: expected 400 ResourceNotFound, got {status} \
                 with {refused:?}"
            );
            assert_eq!(
                message(&refused),
                expected_message(region, "stored-experiment"),
                "case {case:?}/{operation}: got {:?}",
                message(&refused)
            );
        }
    }
}

/// RULE: the refusal is for an experiment that is ABSENT, so an experiment the account does
/// hold is served — whatever case the request spells its name in (`ExperimentName` is "not
/// case-sensitive", smithy_query operation=CreateExperiment).
#[tokio::test]
async fn a_stored_experiment_is_served_rather_than_refused() {
    let region = "us-east-1";
    let emulator = seeded(region).await;

    for (row, spelling) in ["stored-experiment", "STORED-EXPERIMENT", "Stored-Experiment"]
        .into_iter()
        .enumerate()
    {
        let (status, described) = emulator
            .call_in(
                region,
                "DescribeExperiment",
                &json!({ "ExperimentName": spelling }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {spelling:?}: expected HTTP 200 from DescribeExperiment, got {status} \
             with {described:?}"
        );
        assert_eq!(
            str_field(&described, "ExperimentArn"),
            experiment_arn(region, "stored-experiment"),
            "case {spelling:?}: ExperimentArn, got {described:?}"
        );

        // The parent resolves under the same spellings, so `CreateTrial` is served too.
        // The trial name differs per row, because a trial name already in use is refused
        // whatever spelling of the parent the request carried.
        let trial = format!("trial-for-row-{row}");
        let created = emulator
            .ok(
                "CreateTrial",
                &json!({ "TrialName": trial, "ExperimentName": spelling }),
            )
            .await;
        assert_eq!(
            str_field(&created, "TrialArn"),
            format!("arn:aws:sagemaker:{region}:{ACCOUNT}:experiment-trial/{trial}"),
            "case {spelling:?}: TrialArn, got {created:?}"
        );
    }
}
