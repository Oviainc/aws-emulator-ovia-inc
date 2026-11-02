// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for how SageMaker WORDS the refusal of a trial component the
//! account does not hold.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, via the
//! harness `inference_deployment_lifecycle` declares, so what these tests exercise is what
//! a real SDK client gets.
//!
//! The rule asserted here is ONE sentence over a TABLE of inputs rather than one recorded
//! call: every operation that resolves a trial component reports one the account does not
//! hold as `Trial component '<identifier>' does not exist.` — SINGLE QUOTED, the
//! identifier being the requested name resolved to the `experiment-trial-component` ARN of
//! the Region the request signed for, or the ARN itself when the request carried one that
//! denotes no component here — with the modelled `ResourceNotFound` and HTTP 400. The
//! quotes are what delimit the identifier, so a consumer can cut it back out of the
//! refusal, and the three experiment-tracking entities word the sentence identically, so a
//! client parses the trial component, its trial and their experiment uniformly.

use serde_json::{json, Value};

use crate::inference_deployment_lifecycle::{error_code, message, str_field, Emulator, ACCOUNT};

/// The ARN resource kinds the experiment-tracking entities carry, from their modelled ARN
/// patterns (`TrialComponentArn`, `TrialArn`, `ExperimentArn`; smithy_query
/// operation=DescribeTrialComponent / DescribeTrial / DescribeExperiment), spelled out here
/// so the tests pin the wire form rather than echoing the constants under test.
const COMPONENT_KIND: &str = "experiment-trial-component";
const TRIAL_KIND: &str = "experiment-trial";
const EXPERIMENT_KIND: &str = "experiment";

/// The other entity kinds an `ExperimentEntityNameOrArn` may carry, from
/// `TrialComponentName`'s modelled pattern
/// (`^(arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:(experiment|experiment-trial|experiment-trial-component|artifact|action|context)\/)?…`,
/// smithy_query operation=DescribeTrialComponent) — an ARN of any of these is a
/// well-formed identifier that denotes no trial component.
const OTHER_ENTITY_KINDS: [&str; 5] = [
    "experiment",
    "experiment-trial",
    "artifact",
    "action",
    "context",
];

/// A `arn:aws:sagemaker:<region>:<account>:<kind>/<name>` ARN, as this service mints one.
fn arn(region: &str, kind: &str, name: &str) -> String {
    format!("arn:aws:sagemaker:{region}:{ACCOUNT}:{kind}/{name}")
}

/// The whole sentence the refusal must carry for the component `identifier` denotes.
fn expected_message(identifier: &str) -> String {
    format!("Trial component '{identifier}' does not exist.")
}

/// Every operation that resolves a trial component BY NAME (`ExperimentEntityName`), with
/// the request body each takes.
///
/// The membership operations resolve the trial first, so `trial` names one that does exist:
/// the component is the only end missing in these rows.
fn component_readers(component: &str, trial: &str) -> Vec<(&'static str, Value)> {
    vec![
        ("DescribeTrialComponent", json!({ "TrialComponentName": component })),
        ("DeleteTrialComponent", json!({ "TrialComponentName": component })),
        (
            "AssociateTrialComponent",
            json!({ "TrialComponentName": component, "TrialName": trial }),
        ),
        (
            "DisassociateTrialComponent",
            json!({ "TrialComponentName": component, "TrialName": trial }),
        ),
    ]
}

/// An emulator holding one experiment, one trial (`stored-trial`) and one trial component
/// (`stored-component`), all created in `region`.
async fn seeded(region: &str) -> Emulator {
    let emulator = Emulator::new();
    for (target, body) in [
        ("CreateExperiment", json!({ "ExperimentName": "stored-experiment" })),
        (
            "CreateTrial",
            json!({ "TrialName": "stored-trial", "ExperimentName": "stored-experiment" }),
        ),
        (
            "CreateTrialComponent",
            json!({ "TrialComponentName": "stored-component" }),
        ),
    ] {
        let (status, created) = emulator.call_in(region, target, &body).await;
        assert_eq!(
            status, 200,
            "seeding {target} in {region}: expected HTTP 200, got {status} with {created:?}"
        );
    }
    emulator
}

/// RULE: every operation that resolves a trial component reports one the account does not
/// hold as `Trial component '<resolved ARN>' does not exist.` — the requested
/// `TrialComponentName` resolved to the `experiment-trial-component` ARN of the Region the
/// request signed for, single quoted — with the modelled `ResourceNotFound` and HTTP 400.
#[tokio::test]
async fn every_reader_of_a_missing_trial_component_reports_the_quoted_resolved_arn() {
    // Rows differ along the signed Region, the shape of the requested name (the published
    // `ExperimentEntityName` bounds `len 1..120` and its pattern
    // `^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,119}$`, smithy_query operation=DeleteTrialComponent:
    // one character, 120 characters, a digit-leading name, a mixed-case name, a
    // hyphen-free name) and why the component is missing (never created, a near miss of
    // the stored one, the name of a stored resource of ANOTHER kind).
    let rows: [(&str, &str, String); 9] = [
        ("never created", "us-east-1", "component-absent".to_string()),
        ("another region", "eu-west-1", "component-absent".to_string()),
        ("a third region", "ap-south-1", "component-absent".to_string()),
        ("the shortest name", "us-west-2", "A".to_string()),
        ("the longest name", "us-east-1", "c".repeat(120)),
        ("a digit-leading name", "eu-central-1", "9thcomponent".to_string()),
        ("a mixed-case name", "us-east-1", "Absent-Component-9".to_string()),
        (
            "a near miss of the stored component",
            "us-east-1",
            "stored-components".to_string(),
        ),
        // A trial is not a trial component, so its name resolves no component.
        ("the name of the stored trial", "us-east-1", "stored-trial".to_string()),
    ];

    for (case, region, name) in rows {
        // One emulator per row: every call below is a REFUSAL, and a refusal writes
        // nothing, so the four readers of the row share the same seeded store.
        let emulator = seeded(region).await;
        for (operation, body) in component_readers(&name, "stored-trial") {
            let (status, refused) = emulator.call_in(region, operation, &body).await;
            assert_eq!(
                (status, error_code(&refused).as_str()),
                (400, "ResourceNotFound"),
                "case {case:?}/{operation}: expected 400 ResourceNotFound, got {status} \
                 with {refused:?}"
            );
            let reported = message(&refused);
            let component_arn = arn(region, COMPONENT_KIND, &name);
            assert_eq!(
                reported,
                expected_message(&component_arn),
                "case {case:?}/{operation}: expected the resolved ARN single-quoted, got \
                 {reported:?}"
            );
        }
    }
}

/// RULE: the reader takes the component's ARN as well as its name
/// (`ExperimentEntityNameOrArn`), and an identifier that arrived as an ARN denoting no
/// component here is reported AS IT ARRIVED — the same sentence, so one parser serves both
/// forms.
#[tokio::test]
async fn a_missing_component_named_by_an_arn_is_reported_by_that_arn() {
    let region = "us-east-1";
    let emulator = seeded(region).await;

    // Rows differ along what the ARN denotes: a component of this account and Region that
    // was never created, the stored component's name under ANOTHER Region's ARN, and an
    // ARN of each other entity kind the modelled pattern admits.
    let mut rows: Vec<(String, String)> = vec![
        (
            "an absent component's ARN".to_string(),
            arn(region, COMPONENT_KIND, "component-absent"),
        ),
        (
            "the stored component under another Region's ARN".to_string(),
            arn("eu-west-1", COMPONENT_KIND, "stored-component"),
        ),
        (
            "another account's component ARN".to_string(),
            format!("arn:aws:sagemaker:{region}:210987654321:{COMPONENT_KIND}/stored-component"),
        ),
    ];
    for kind in OTHER_ENTITY_KINDS {
        rows.push((format!("an ARN of kind {kind:?}"), arn(region, kind, "stored-trial")));
    }

    for (case, identifier) in rows {
        let (status, refused) = emulator
            .call_in(
                region,
                "DescribeTrialComponent",
                &json!({ "TrialComponentName": identifier }),
            )
            .await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "ResourceNotFound"),
            "case {case:?}: expected 400 ResourceNotFound, got {status} with {refused:?}"
        );
        let reported = message(&refused);
        assert_eq!(
            reported,
            expected_message(&identifier),
            "case {case:?}: expected the ARN that arrived single-quoted, got {reported:?}"
        );
    }
}

/// RULE: a component that was DELETED is as missing as one nobody created, and the three
/// experiment-tracking entities word the sentence identically — each naming its own entity
/// label and its own ARN kind — so a caller parses the three refusals of a torn-down
/// experiment uniformly.
#[tokio::test]
async fn a_torn_down_experiment_reports_its_three_entities_with_the_same_sentence() {
    for region in ["us-east-1", "eu-west-1"] {
        let emulator = seeded(region).await;

        // The lifecycle the entities are torn down through: a component is part of the
        // trial, then is not, then none of the three exists.
        for (target, body) in [
            (
                "AssociateTrialComponent",
                json!({ "TrialComponentName": "stored-component", "TrialName": "stored-trial" }),
            ),
            (
                "DisassociateTrialComponent",
                json!({ "TrialComponentName": "stored-component", "TrialName": "stored-trial" }),
            ),
            (
                "DeleteTrialComponent",
                json!({ "TrialComponentName": "stored-component" }),
            ),
            ("DeleteTrial", json!({ "TrialName": "stored-trial" })),
            ("DeleteExperiment", json!({ "ExperimentName": "stored-experiment" })),
        ] {
            let (status, answered) = emulator.call_in(region, target, &body).await;
            assert_eq!(
                status, 200,
                "case {region:?}/{target}: expected HTTP 200, got {status} with {answered:?}"
            );
        }

        // Each row: the reader, the member naming the entity, the entity's name, the
        // sentence's entity label and the ARN kind the label's resource carries.
        let rows: [(&str, &str, &str, &str, &str); 3] = [
            (
                "DescribeTrialComponent",
                "TrialComponentName",
                "stored-component",
                "Trial component",
                COMPONENT_KIND,
            ),
            ("DescribeTrial", "TrialName", "stored-trial", "Trial", TRIAL_KIND),
            (
                "DescribeExperiment",
                "ExperimentName",
                "stored-experiment",
                "Experiment",
                EXPERIMENT_KIND,
            ),
        ];

        for (operation, member, name, label, kind) in rows {
            let (status, refused) = emulator
                .call_in(region, operation, &json!({ member: name }))
                .await;
            assert_eq!(
                (status, error_code(&refused).as_str()),
                (400, "ResourceNotFound"),
                "case {region:?}/{operation}: expected 400 ResourceNotFound, got {status} \
                 with {refused:?}"
            );
            let reported = message(&refused);
            let entity_arn = arn(region, kind, name);
            assert_eq!(
                reported,
                format!("{label} '{entity_arn}' does not exist."),
                "case {region:?}/{operation}: got {reported:?}"
            );
        }
    }
}

/// RULE: the refusal is for a component that is ABSENT, so a component the account does
/// hold is SERVED — by name in whatever case the request spells it (`TrialComponentName`
/// "is not case-sensitive", smithy_query operation=CreateTrialComponent) and by its ARN.
#[tokio::test]
async fn a_stored_trial_component_is_served_rather_than_refused() {
    let region = "us-west-2";
    let emulator = seeded(region).await;
    let component_arn = arn(region, COMPONENT_KIND, "stored-component");

    for identifier in [
        "stored-component".to_string(),
        "STORED-COMPONENT".to_string(),
        "Stored-Component".to_string(),
        component_arn.clone(),
    ] {
        let (status, described) = emulator
            .call_in(
                region,
                "DescribeTrialComponent",
                &json!({ "TrialComponentName": identifier }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {identifier:?}: expected HTTP 200 from DescribeTrialComponent, got \
             {status} with {described:?}"
        );
        assert_eq!(
            str_field(&described, "TrialComponentArn"),
            component_arn,
            "case {identifier:?}: TrialComponentArn, got {described:?}"
        );
        assert_eq!(
            str_field(&described, "TrialComponentName"),
            "stored-component",
            "case {identifier:?}: TrialComponentName, got {described:?}"
        );
    }
}
