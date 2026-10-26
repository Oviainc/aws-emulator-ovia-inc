// SPDX-License-Identifier: CC0-1.0

//! How AppConfig WORDS the refusals the experiment-run paths produce: the conflict a start
//! meets on a definition that is already running an experiment, the conflict an update of
//! such a definition meets, and the not-found a run number nothing ever issued meets.
//!
//! Three rules are under test, each one level more general than any single request:
//!
//! 1. A start refused because the resource it addresses already has work in flight names
//!    that resource as the REQUEST addressed it, followed by the fixed sentence saying
//!    what must finish before a start is accepted — and discloses nothing about the work
//!    already in flight, neither its number nor its status.
//! 2. A mutation refused by the addressed record's own lifecycle names the record as the
//!    request addressed it AND the status the record currently holds, followed by the fixed
//!    sentence saying what must finish first, so a caller can tell this refusal from the
//!    other lifecycle refusals of the same operation.
//! 3. A `ResourceNotFoundException` message is the looked-up KIND's own AppConfig
//!    template: the kind spelled as that template spells it, each identifier under that
//!    template's label for it and in that template's order, then the account clause — and
//!    every operation reaching the same lookup words it identically.
//!
//! The tables span what those rules generalise over rather than one recorded request:
//! whether the application and the definition are addressed by minted id or by name (both
//! are `Identifier` members, documented as the resource's "ID or name", from
//! `smithy_query(query="operation", operation="StartExperimentRun")`), which optional
//! members the refused request carries and their values at the published bounds
//! (`Description` len 0..1024, `NullablePercentage` range 0..100), WHICH run number is
//! missing — the next one, a far one and the 32-bit upper end of `PositiveInteger`
//! (range min 1, from `smithy_query(query="operation", operation="GetExperimentRun")`) —
//! and WHICH of the three operations that reach the run lookup made the call.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::appconfig_workflow::{account, json_call, router, Answer};
use crate::experiment_definitions::{get_definition, scope, Scope};
use crate::resource_tags::pct;
use crate::start_experiment_run::{assert_refused, create_test_definition, get_run, id_of, start_run};

/// `PATCH /applications/{ApplicationIdentifier}/experimentdefinitions/{ExperimentDefinitionIdentifier}`.
async fn update_definition(
    router: &MultiServiceRouter,
    application: &str,
    definition: &str,
    patch: &Value,
) -> Answer {
    json_call(
        router,
        "PATCH",
        &format!(
            "/applications/{}/experimentdefinitions/{}",
            pct(application),
            pct(definition)
        ),
        patch,
    )
    .await
}

/// `PATCH .../experimentruns/{Run}/update`.
async fn update_run(
    router: &MultiServiceRouter,
    application: &str,
    definition: &str,
    run: i64,
    patch: &Value,
) -> Answer {
    json_call(
        router,
        "PATCH",
        &format!(
            "/applications/{}/experimentdefinitions/{}/experimentruns/{run}/update",
            pct(application),
            pct(definition)
        ),
        patch,
    )
    .await
}

/// `GET .../experimentruns/{Run}/events`.
async fn list_run_events(
    router: &MultiServiceRouter,
    application: &str,
    definition: &str,
    run: i64,
) -> Answer {
    json_call(
        router,
        "GET",
        &format!(
            "/applications/{}/experimentdefinitions/{}/experimentruns/{run}/events",
            pct(application),
            pct(definition)
        ),
        &Value::Null,
    )
    .await
}

/// The `Message` of a refusal, or a panic carrying the body a caller needs to see.
fn message(case: &str, answer: &Answer) -> String {
    answer.string("Message").unwrap_or_else(|| {
        panic!(
            "case {case:?}: the refusal carried no Message: {}",
            String::from_utf8_lossy(&answer.body)
        )
    })
}

/// Assert a refusal's whole message, and that it carries none of the text its template
/// withholds.
fn assert_message(case: &str, answer: &Answer, expected: &str, withheld: &[&str]) {
    let actual = message(case, answer);
    assert_eq!(
        actual, expected,
        "case {case:?}: expected message {expected:?}, got {actual:?}"
    );
    for absent in withheld {
        assert!(
            !actual.contains(absent),
            "case {case:?}: the template withholds {absent:?}, got {actual:?}"
        );
    }
}

/// Start the first run of a fresh definition, and answer the definition's minted id beside
/// the name the create gave it — the two forms a later request can address it by.
async fn running_definition(
    router: &MultiServiceRouter,
    scope: &Scope,
    name: &str,
) -> String {
    let definition = create_test_definition(router, scope, name).await;
    let definition_id = id_of(name, &definition);
    let started = start_run(router, &scope.application, &definition_id, &json!({})).await;
    assert_eq!(
        started.status,
        201,
        "case {name:?}: the run that makes the definition busy must start, got {} / {}",
        started.status,
        String::from_utf8_lossy(&started.body)
    );
    definition_id
}

/// Rule 1: a start aimed at a definition whose run is still in flight is refused with that
/// definition's own template — the identifier the request sent plus the fixed remediation
/// sentence — and the refusal says nothing about the run that holds the definition.
#[tokio::test]
async fn a_start_on_a_busy_definition_names_the_definition_and_withholds_the_run() {
    const LABEL: &str = "busy";
    let router = router();
    let scope = scope(&router, LABEL).await;
    let application_name = format!("app-{LABEL}");

    // Rows differ along which identifier form addresses each resource and which optional
    // members the refused start carries, at both published bounds of each.
    for (index, (case, application_by_name, definition_by_name, body)) in [
        (
            "both addressed by minted id, nothing optional",
            false,
            false,
            json!({}),
        ),
        (
            "the definition addressed by name, with a description",
            false,
            true,
            json!({ "Description": "the start that lost" }),
        ),
        (
            "the application addressed by name, exposing none of the audience",
            true,
            false,
            json!({ "ExposurePercentage": 0.0 }),
        ),
        (
            "both addressed by name, an empty description and the whole audience",
            true,
            true,
            json!({ "Description": "", "ExposurePercentage": 100.0 }),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let name = format!("{LABEL}-definition-{index}");
        let definition_id = running_definition(&router, &scope, &name).await;
        let application = if application_by_name {
            application_name.clone()
        } else {
            scope.application.clone()
        };
        let addressed = if definition_by_name {
            name.clone()
        } else {
            definition_id.clone()
        };

        let refused = start_run(&router, &application, &addressed, &body).await;
        assert_refused(case, &refused, 409, "ConflictException");
        assert_message(
            case,
            &refused,
            &format!(
                "Experiment definition {addressed} already has a running experiment. The \
                 current experiment run must be stopped before a run can be started."
            ),
            // The run in flight is not part of this refusal: neither the number it took
            // nor the status it holds is disclosed.
            &["RUNNING", "run 1", "active experiment run"],
        );

        // And the refused start issued no number: the definition still holds one run.
        let second = get_run(&router, &application, &addressed, 2).await;
        assert_refused(case, &second, 404, "ResourceNotFoundException");
    }
}

/// Rule 2: while a definition has a run in flight, an update of it is refused with the
/// request's identifier and the status the definition itself reports, and writes nothing.
#[tokio::test]
async fn an_update_of_a_busy_definition_names_the_status_that_blocked_it() {
    const LABEL: &str = "lifecycle";
    let router = router();
    let scope = scope(&router, LABEL).await;
    let application_name = format!("app-{LABEL}");
    let name = format!("{LABEL}-definition");
    // The status that refuses the update is reached through the wire — a real start — so
    // the status the refusal names is the one the definition reports to a reader.
    let definition_id = running_definition(&router, &scope, &name).await;

    let before = get_definition(&router, &scope.application, &definition_id).await;
    assert_eq!(
        before.status,
        200,
        "the definition under test must read back, got {} / {}",
        before.status,
        String::from_utf8_lossy(&before.body)
    );
    let status = before
        .string("Status")
        .expect("the definition must report a Status");
    let document = before.json();

    // Rows differ along which identifier form addresses each resource and which members
    // the refused patch carries — none of them is written while a run is in flight.
    for (case, application_by_name, definition_by_name, patch) in [
        (
            "both addressed by minted id, a new hypothesis",
            false,
            false,
            json!({ "Hypothesis": "must not be stored" }),
        ),
        (
            "the definition addressed by name, a new audience rule",
            false,
            true,
            json!({ "AudienceRule": "true" }),
        ),
        (
            "the application addressed by name, an empty patch",
            true,
            false,
            json!({}),
        ),
        (
            "both addressed by name, every writable member",
            true,
            true,
            json!({
                "Hypothesis": "nor this",
                "AudienceRule": "false",
                "AudienceDescription": "nor this",
                "LaunchCriteria": "nor this",
            }),
        ),
    ] {
        let application = if application_by_name {
            application_name.clone()
        } else {
            scope.application.clone()
        };
        let addressed = if definition_by_name {
            name.clone()
        } else {
            definition_id.clone()
        };

        let refused = update_definition(&router, &application, &addressed, &patch).await;
        assert_refused(case, &refused, 409, "ConflictException");
        assert_message(
            case,
            &refused,
            &format!(
                "Cannot update experiment definition {addressed} with status {status}. Stop \
                 the current experiment run before updating the experiment definition."
            ),
            &[],
        );

        let after = get_definition(&router, &scope.application, &definition_id).await;
        assert_eq!(
            after.json(),
            document,
            "case {case:?}: a refused update must write nothing, expected {document:?}, got \
             {:?}",
            after.json()
        );
    }
}

/// Rule 3: a run number the definition never issued is refused with the experiment run's
/// own template — definition identifier first, then the application, then the run, and the
/// account the lookup was scoped to — whichever operation reached the lookup.
#[tokio::test]
async fn a_run_number_that_addresses_nothing_states_the_experiment_runs_template() {
    const LABEL: &str = "missing";
    let router = router();
    let scope = scope(&router, LABEL).await;
    let application_name = format!("app-{LABEL}");
    let account = account();
    let name = format!("{LABEL}-definition");
    let definition_id = running_definition(&router, &scope, &name).await;

    // The positive control: the number the start DID issue addresses a run, so what the
    // rows below exercise is the failing lookup and nothing else.
    let found = get_run(&router, &scope.application, &definition_id, 1).await;
    assert_eq!(
        found.status,
        200,
        "the run the start issued must be found, got {} / {}",
        found.status,
        String::from_utf8_lossy(&found.body)
    );

    for run in [2_i64, 10_000, i64::from(i32::MAX)] {
        for (shape, application, addressed) in [
            (
                "both addressed by minted id",
                scope.application.clone(),
                definition_id.clone(),
            ),
            (
                "the definition addressed by name",
                scope.application.clone(),
                name.clone(),
            ),
            (
                "the application addressed by name",
                application_name.clone(),
                definition_id.clone(),
            ),
            (
                "both addressed by name",
                application_name.clone(),
                name.clone(),
            ),
        ] {
            let expected = format!(
                "Experiment run with Experiment Definition Id {addressed}, Application Id \
                 {application}, and Experiment Run {run} could not be found for account \
                 {account}."
            );
            for (operation, answer) in [
                (
                    "GetExperimentRun",
                    get_run(&router, &application, &addressed, run).await,
                ),
                (
                    "UpdateExperimentRun",
                    update_run(
                        &router,
                        &application,
                        &addressed,
                        run,
                        &json!({ "Description": "nothing to update" }),
                    )
                    .await,
                ),
                (
                    "ListExperimentRunEvents",
                    list_run_events(&router, &application, &addressed, run).await,
                ),
            ] {
                let case = format!("{operation} with {shape} and run {run}");
                assert_refused(&case, &answer, 404, "ResourceNotFoundException");
                assert_message(
                    &case,
                    &answer,
                    &expected,
                    // The template spells the kind with a lowercase `run` and labels the
                    // values as ids, so neither the capitalised kind nor the member-name
                    // labels appear.
                    &[
                        "Experiment Run with",
                        "Application Identifier",
                        "Experiment Definition Identifier",
                    ],
                );
            }
        }
    }
}
