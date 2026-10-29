// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for how SageMaker WORDS the refusal of a trial the account does
//! not hold.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, via the
//! harness `inference_deployment_lifecycle` declares, so what these tests exercise is what
//! a real SDK client gets.
//!
//! The rule asserted here is ONE sentence over a TABLE of inputs rather than one recorded
//! call: every operation that resolves a trial reports one the account does not hold as
//! `Trial '<resolved ARN>' does not exist.` — the requested `TrialName` resolved to the
//! trial ARN of the Region the request signed for, SINGLE QUOTED — with the modelled
//! `ResourceNotFound` and HTTP 400. The quotes are what delimit the ARN, so a consumer can
//! cut the identifier back out of the refusal.

use serde_json::{json, Value};

use crate::inference_deployment_lifecycle::{error_code, message, str_field, Emulator, ACCOUNT};

/// The ARN resource kind `TrialArn` is modelled with
/// (`^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:experiment-trial/`, smithy_query
/// operation=DescribeTrial), spelled out here so the test pins the wire form rather than
/// echoing the constant under test.
const TRIAL_KIND: &str = "experiment-trial";

/// The ARN resource kind `TrialComponentArn` is modelled with
/// (`^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:experiment-trial-component/`,
/// smithy_query operation=AssociateTrialComponent).
const COMPONENT_KIND: &str = "experiment-trial-component";

/// The trial ARN a `TrialName` resolves to in `region`, as this service mints one.
fn trial_arn(region: &str, name: &str) -> String {
    format!("arn:aws:sagemaker:{region}:{ACCOUNT}:{TRIAL_KIND}/{name}")
}

/// The whole sentence the refusal must carry for `name` in `region`.
fn expected_message(region: &str, name: &str) -> String {
    format!("Trial '{}' does not exist.", trial_arn(region, name))
}

/// Every operation that resolves a trial by name, with the request body each takes.
///
/// The membership operations resolve the trial BEFORE the component, so the trial is the
/// end they report; `component` names a component that does exist, so the trial is the
/// only thing missing in these rows.
fn trial_readers(trial: &str, component: &str) -> Vec<(&'static str, Value)> {
    vec![
        ("DescribeTrial", json!({ "TrialName": trial })),
        ("DeleteTrial", json!({ "TrialName": trial })),
        (
            "AssociateTrialComponent",
            json!({ "TrialName": trial, "TrialComponentName": component }),
        ),
        (
            "DisassociateTrialComponent",
            json!({ "TrialName": trial, "TrialComponentName": component }),
        ),
    ]
}

/// An emulator holding one experiment, one trial (`stored-trial`) and one trial component
/// (`stored-component`), all created in `region`.
async fn seeded(region: &str) -> Emulator {
    let emulator = Emulator::new();
    let (status, created) = emulator
        .call_in(region, "CreateExperiment", &json!({ "ExperimentName": "stored-experiment" }))
        .await;
    assert_eq!(
        status, 200,
        "seeding the experiment: expected HTTP 200, got {status} with {created:?}"
    );
    let (status, created) = emulator
        .call_in(
            region,
            "CreateTrial",
            &json!({ "TrialName": "stored-trial", "ExperimentName": "stored-experiment" }),
        )
        .await;
    assert_eq!(
        status, 200,
        "seeding the trial: expected HTTP 200, got {status} with {created:?}"
    );
    let (status, created) = emulator
        .call_in(region, "CreateTrialComponent", &json!({ "TrialComponentName": "stored-component" }))
        .await;
    assert_eq!(
        status, 200,
        "seeding the component: expected HTTP 200, got {status} with {created:?}"
    );
    emulator
}

/// The identifier a consumer cuts out of `reported` by taking what the single quotes
/// delimit, or `None` when the sentence carries no such pair.
fn quoted_identifier(reported: &str) -> Option<&str> {
    let (_, after) = reported.split_once('\'')?;
    let (inside, _) = after.rsplit_once('\'')?;
    Some(inside)
}

/// RULE: every operation that resolves a trial reports a trial the account does not hold
/// as `Trial '<resolved ARN>' does not exist.` — the requested `TrialName` resolved to the
/// `experiment-trial` ARN of the Region the request signed for, single quoted — with the
/// modelled `ResourceNotFound` and HTTP 400.
#[tokio::test]
async fn every_reader_of_a_missing_trial_reports_the_quoted_resolved_arn() {
    // Rows differ along the signed Region, the shape of the requested name (the published
    // `ExperimentEntityName` bounds `len 1..120` and its pattern
    // `^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,119}$`, smithy_query operation=DescribeTrial: one
    // character, 120 characters, a digit-leading name, a mixed-case name, hyphen-free) and
    // why the trial is missing (never created, a near miss of the stored one, the name of a
    // stored resource of ANOTHER kind).
    let rows: [(&str, &str, String); 9] = [
        ("never created", "us-east-1", "trial-absent".to_string()),
        ("another region", "eu-west-1", "trial-absent".to_string()),
        ("a third region", "ap-south-1", "trial-absent".to_string()),
        ("the shortest name", "us-west-2", "A".to_string()),
        ("the longest name", "us-east-1", "n".repeat(120)),
        ("a digit-leading name", "eu-central-1", "9thtrial".to_string()),
        ("a mixed-case name", "us-east-1", "Absent-Trial-9".to_string()),
        ("a near miss of the stored trial", "us-east-1", "stored-trials".to_string()),
        // A component is not a trial, so its name resolves no trial.
        ("the name of a stored component", "us-east-1", "stored-component".to_string()),
    ];

    for (case, region, name) in rows {
        // One emulator per row: every row below is a REFUSAL, and a refusal writes
        // nothing, so the four readers of the row share the same seeded store.
        let emulator = seeded(region).await;
        for (operation, body) in trial_readers(&name, "stored-component") {
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
                Some(trial_arn(region, &name).as_str()),
                "case {case:?}/{operation}: the quotes must delimit the ARN, got \
                 {reported:?}"
            );
        }
    }
}

/// RULE: the same sentence states a trial that was deleted — a released name is as missing
/// as one nobody created — and the trial that still exists is unaffected.
#[tokio::test]
async fn a_deleted_trial_is_reported_the_same_way_as_one_never_created() {
    for (case, region) in [("after a delete", "us-east-1"), ("in another region", "eu-west-1")] {
        let emulator = seeded(region).await;
        let deleted = emulator
            .call_in(region, "DeleteTrial", &json!({ "TrialName": "stored-trial" }))
            .await;
        assert_eq!(
            deleted.0, 200,
            "case {case:?}: expected HTTP 200 from DeleteTrial, got {deleted:?}"
        );

        for (operation, body) in trial_readers("stored-trial", "stored-component") {
            let (status, refused) = emulator.call_in(region, operation, &body).await;
            assert_eq!(
                (status, error_code(&refused).as_str()),
                (400, "ResourceNotFound"),
                "case {case:?}/{operation}: expected 400 ResourceNotFound, got {status} \
                 with {refused:?}"
            );
            assert_eq!(
                message(&refused),
                expected_message(region, "stored-trial"),
                "case {case:?}/{operation}: got {:?}",
                message(&refused)
            );
        }
    }
}

/// RULE: the refusal is for a trial that is ABSENT, so a trial the account does hold is
/// served — whatever case the request spells its name in (`TrialName` "is not
/// case-sensitive", smithy_query operation=CreateTrial).
#[tokio::test]
async fn a_stored_trial_is_served_rather_than_refused() {
    let region = "us-east-1";
    let emulator = seeded(region).await;

    for spelling in ["stored-trial", "STORED-TRIAL", "Stored-Trial"] {
        let (status, described) = emulator
            .call_in(region, "DescribeTrial", &json!({ "TrialName": spelling }))
            .await;
        assert_eq!(
            status, 200,
            "case {spelling:?}: expected HTTP 200 from DescribeTrial, got {status} with \
             {described:?}"
        );
        assert_eq!(
            str_field(&described, "TrialArn"),
            trial_arn(region, "stored-trial"),
            "case {spelling:?}: TrialArn, got {described:?}"
        );
    }

    let associated = emulator
        .ok(
            "AssociateTrialComponent",
            &json!({ "TrialName": "stored-trial", "TrialComponentName": "stored-component" }),
        )
        .await;
    assert_eq!(
        str_field(&associated, "TrialArn"),
        trial_arn(region, "stored-trial"),
        "case \"a stored trial\": TrialArn, got {associated:?}"
    );
    assert_eq!(
        str_field(&associated, "TrialComponentArn"),
        format!("arn:aws:sagemaker:{region}:{ACCOUNT}:{COMPONENT_KIND}/stored-component"),
        "case \"a stored trial\": TrialComponentArn, got {associated:?}"
    );
}
