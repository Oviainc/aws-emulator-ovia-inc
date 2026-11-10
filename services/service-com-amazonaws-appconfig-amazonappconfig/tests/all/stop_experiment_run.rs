// SPDX-License-Identifier: CC0-1.0

//! `StopExperimentRun`, and the reads that prove what it wrote — `GetExperimentRun`,
//! `GetExperimentDefinition` and `ListExperimentRunEvents` — end to end over HTTP.
//!
//! The rules under test:
//!
//! * stopping a run ENDS it: the run reports the finished status, the instant it ended and
//!   that same instant as the one the record was last written at, while every member the
//!   start wrote — the description, the exposure, the overrides and the snapshot of the
//!   definition — is reported back untouched;
//! * the conclusion is the caller's: a stop that carries `Result` stores it, a stop that
//!   carries none leaves the record's own, and `DeploymentParameters` is not a member of
//!   the run, so nothing of it is reported on one;
//! * what the stop wrote is what the readers answer: `GetExperimentRun` reports the same
//!   document, the run's timeline gains one entry for the stop — which heads the
//!   most-recent-first feed, ahead of the one the start wrote — and the definition leaves
//!   the status it holds while an experiment is in flight
//!   — so a new run of it may be started again;
//! * a run the experiment has already finished with takes no second stop: an experiment
//!   run never returns to running, so it is HTTP 400 `BadRequestException` naming the run
//!   and the definition it was addressed under, and it writes nothing;
//! * a stop ends the ONE run its path addresses: the runs the definition already finished
//!   with keep the documents they finished with, and a run of another definition keeps
//!   running;
//! * an application, a definition or a run number that addresses nothing is HTTP 404
//!   `ResourceNotFoundException`, and writes nothing.
//!
//! The tables span what those rules generalise over rather than one recorded request:
//! which members `Result` carries and their values at the published bounds
//! (`ExecutiveSummary`, `ReasonsToLaunch`, `ReasonsNotToLaunch`, each `Description` len
//! 0..1024, from `smithy_query(query="shape", shape="ExperimentRunResult")`), whether the
//! stop carries `DeploymentParameters`, what the start that is being stopped carried
//! (`Description` len 0..1024 and `NullablePercentage` range 0..100, at both bounds, and
//! treatment overrides), whether the resources are addressed by minted id or by name (both
//! are `Identifier` members, documented as the resource's "ID or name"), and WHICH level
//! of the path — application, definition or run number — resolves to nothing.

use serde_json::{json, Value};
use service_com_amazonaws_appconfig_amazonappconfig::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::appconfig_workflow::{create_application, json_call, router, Answer};
use crate::experiment_definitions::{get_definition, scope, Scope};
use crate::resource_tags::{filler, pct};
use crate::start_experiment_run::{
    assert_refused, create_test_definition, get_run, id_of, start_run, string_map,
};
use crate::update_experiment_definition::instant;

/// `PATCH .../experimentruns/{Run}/stop`, this operation's modelled URI
/// (`smithy_query(query="operation", operation="StopExperimentRun")`). `pub(crate)`
/// because the delete suite stops a run to reach the state its positive control needs.
pub(crate) async fn stop_run(
    router: &MultiServiceRouter,
    application: &str,
    definition: &str,
    run: i64,
    body: &Value,
) -> Answer {
    json_call(
        router,
        "PATCH",
        &format!(
            "/applications/{}/experimentdefinitions/{}/experimentruns/{run}/stop",
            pct(application),
            pct(definition)
        ),
        body,
    )
    .await
}

/// `GET .../experimentruns/{Run}/events`.
async fn list_events(
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

/// The `Items` of a listing, or a panic carrying the body.
fn items(case: &str, answer: &Answer, member: &str) -> Vec<Value> {
    let body = answer.json();
    body.get(member)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected {member} in {body}"))
}

/// The `EventType` of every entry a run's timeline reports, in order.
fn event_types(case: &str, answer: &Answer) -> Vec<String> {
    items(case, answer, "Items")
        .into_iter()
        .map(|event| {
            event
                .get("EventType")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: an event reported no EventType: {event}"))
                .to_string()
        })
        .collect()
}

/// Start one run of a fresh definition, and answer the definition's id, the name it was
/// created under and the started run's document.
async fn running(
    router: &MultiServiceRouter,
    case: &str,
    scope: &Scope,
    name: &str,
    start_body: &Value,
) -> (String, Value) {
    let definition = create_test_definition(router, scope, name).await;
    let definition_id = id_of(case, &definition);
    let started = start_run(router, &scope.application, &definition_id, start_body).await;
    assert_eq!(
        started.status,
        201,
        "case {case:?}: the run under test must start, got {} / {}",
        started.status,
        String::from_utf8_lossy(&started.body)
    );
    (definition_id, started.json())
}

/// A stop ends the run, stores the conclusion the caller stated, leaves everything else
/// the start wrote alone, and frees the definition for another run.
#[tokio::test]
async fn a_stop_ends_the_run_and_frees_the_definition() {
    const LABEL: &str = "stop";
    let router = router();
    let scope = scope(&router, LABEL).await;
    let application_name = format!("app-{LABEL}");
    // The published upper bound of every `ExperimentRunResult` member (`Description`, len
    // 0..1024).
    let longest = filler(1024, 7);

    // Rows differ along what the start being stopped carried, which members the stop's
    // `Result` carries and their values at the published bounds (the empty string and the
    // 1024-character maximum), whether the stop also carries `DeploymentParameters`, and
    // whether the stop addresses the application and the definition by id or by name.
    for (index, (case, start_body, stop_body, by_name)) in [
        (
            "a bare start stopped with no result at all",
            json!({}),
            json!({}),
            false,
        ),
        (
            "the whole audience exposed, stopped with an executive summary",
            json!({ "Description": "the launch candidate", "ExposurePercentage": 100.0 }),
            json!({ "Result": { "ExecutiveSummary": "the treatment won" } }),
            false,
        ),
        (
            "overrides assigned, stopped with every reason at the published maximum",
            json!({
                "ExposurePercentage": 12.5,
                "TreatmentOverrides": { "Inline": { "entity-1": "t1", "entity-2": "t1" } },
            }),
            json!({
                "Result": {
                    "ExecutiveSummary": longest,
                    "ReasonsToLaunch": longest,
                    "ReasonsNotToLaunch": longest,
                },
            }),
            true,
        ),
        (
            "none of the audience exposed, stopped with empty reasons and deployment \
             parameters",
            json!({ "Description": "", "ExposurePercentage": 0.0 }),
            json!({
                "Result": { "ExecutiveSummary": "", "ReasonsToLaunch": "" },
                "DeploymentParameters": {
                    "DynamicExtensionParameters": { "p": "v" },
                    "Tags": { "owner": "experiments" },
                },
            }),
            true,
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let name = format!("{LABEL}-definition-{index}");
        let (definition_id, started) = running(&router, case, &scope, &name, &start_body).await;
        let (application, addressed) = if by_name {
            (application_name.clone(), name.clone())
        } else {
            (scope.application.clone(), definition_id.clone())
        };

        let stopped = stop_run(&router, &application, &addressed, 1, &stop_body).await;
        assert_eq!(
            stopped.status,
            200,
            "case {case:?}: expected 200, got {} / {}",
            stopped.status,
            String::from_utf8_lossy(&stopped.body)
        );
        let ended = stopped.json();

        // The run is over: the finished status, the instant it ended, and that instant as
        // the one the record was last written at.
        assert_eq!(
            ended.get("Status").and_then(Value::as_str),
            Some(state::EXPERIMENT_RUN_DONE),
            "case {case:?}: expected the finished status, got {:?}",
            ended.get("Status")
        );
        let started_at = instant(case, "StartedAt", ended.get("StartedAt"));
        let ended_at = instant(case, "EndedAt", ended.get("EndedAt"));
        let updated_at = instant(case, "UpdatedAt", ended.get("UpdatedAt"));
        assert!(
            ended_at >= started_at,
            "case {case:?}: expected EndedAt at or after StartedAt, got {ended_at} and \
             {started_at}"
        );
        assert_eq!(
            updated_at, ended_at,
            "case {case:?}: the stop wrote the record as it ended it, expected UpdatedAt \
             {ended_at}, got {updated_at}"
        );

        // Which run of which definition, and everything the start wrote, are reported
        // exactly as the start reported them.
        for member in [
            "ApplicationId",
            "ExperimentDefinitionId",
            "Run",
            "Description",
            "ExposurePercentage",
            "TreatmentOverrides",
            "ExperimentDefinitionSnapshot",
        ] {
            assert_eq!(
                ended.get(member),
                started.get(member),
                "case {case:?}: expected {member} to be untouched as {:?}, got {:?}",
                started.get(member),
                ended.get(member)
            );
        }
        // The conclusion is the caller's: what the stop stated, and nothing where it
        // stated none.
        assert_eq!(
            ended.get("Result"),
            stop_body.get("Result"),
            "case {case:?}: expected the Result the stop stated {:?}, got {:?}",
            stop_body.get("Result"),
            ended.get("Result")
        );
        // `DeploymentParameters` is an input member with no counterpart on a run.
        assert!(
            ended.get("DeploymentParameters").is_none(),
            "case {case:?}: a run reports no DeploymentParameters, got {ended}"
        );

        // What the stop wrote is what the reader answers.
        let read = get_run(&router, &scope.application, &definition_id, 1).await;
        assert_eq!(
            read.status,
            200,
            "case {case:?}: the stopped run must read back, got {} / {}",
            read.status,
            String::from_utf8_lossy(&read.body)
        );
        assert_eq!(
            read.json(),
            ended,
            "case {case:?}: expected GetExperimentRun to report {ended}, got {}",
            read.json()
        );
        // The timeline gained one entry for the stop, and the listing serves it most
        // recent first, so the stop heads the feed and the start the run began with tails
        // it.
        let events = list_events(&router, &scope.application, &definition_id, 1).await;
        assert_eq!(
            event_types(case, &events),
            vec![
                state::EVENT_RUN_STOPPED.to_string(),
                state::EVENT_RUN_STARTED.to_string(),
            ],
            "case {case:?}: expected the stop then the start on the timeline, got {}",
            events.json()
        );
        let stop_event = items(case, &events, "Items")
            .into_iter()
            .next()
            .unwrap_or_else(|| panic!("case {case:?}: the timeline reported no entries"));
        assert_eq!(
            stop_event.get("TriggeredBy").and_then(Value::as_str),
            Some("USER"),
            "case {case:?}: a caller's stop is triggered by USER, got {stop_event}"
        );
        assert_eq!(
            stop_event.get("ExposurePercentage"),
            ended.get("ExposurePercentage"),
            "case {case:?}: the entry reports the exposure at the time of the event {:?}, got \
             {:?}",
            ended.get("ExposurePercentage"),
            stop_event.get("ExposurePercentage")
        );
        assert_eq!(
            instant(case, "OccurredAt", stop_event.get("OccurredAt")),
            ended_at,
            "case {case:?}: the entry occurred when the run ended, got {stop_event}"
        );

        // The definition has no experiment in flight any more, so it reports the idle
        // status and takes another run — which is numbered after the one that ended.
        let definition = get_definition(&router, &scope.application, &definition_id).await;
        assert_eq!(
            definition.string("Status").as_deref(),
            Some(state::EXPERIMENT_DEFINITION_IDLE),
            "case {case:?}: expected the idle status, got {}",
            definition.json()
        );
        let again = start_run(&router, &application, &addressed, &json!({})).await;
        assert_eq!(
            again.status,
            201,
            "case {case:?}: a stopped definition must take another run, got {} / {}",
            again.status,
            String::from_utf8_lossy(&again.body)
        );
        assert_eq!(
            again.json().get("Run").and_then(Value::as_i64),
            Some(2),
            "case {case:?}: a run number is never reissued, got {}",
            again.json()
        );
    }
}

/// A run the experiment has already finished with takes no second stop: HTTP 400
/// `BadRequestException`, naming the run and the definition the request addressed it under,
/// and nothing is written — the instant it DID end and the conclusion it ended with stand.
#[tokio::test]
async fn a_second_stop_of_a_finished_run_is_refused_and_changes_nothing() {
    const LABEL: &str = "twice";
    let router = router();
    let scope = scope(&router, LABEL).await;
    let application_name = format!("app-{LABEL}");
    let name = format!("{LABEL}-definition");
    let (definition_id, _) = running(
        &router,
        "the run under test",
        &scope,
        &name,
        &json!({ "ExposurePercentage": 50.0 }),
    )
    .await;
    let first = stop_run(
        &router,
        &scope.application,
        &definition_id,
        1,
        &json!({ "Result": { "ExecutiveSummary": "the only conclusion" } }),
    )
    .await;
    assert_eq!(
        first.status,
        200,
        "the run must stop once, got {} / {}",
        first.status,
        String::from_utf8_lossy(&first.body)
    );
    let ended = first.json();

    // Rows differ along how the second stop addresses the run and what it would write:
    // another conclusion, no conclusion, and deployment parameters. None of them lands.
    for (case, by_name, body) in [
        (
            "addressed by id, restating the conclusion",
            false,
            json!({ "Result": { "ExecutiveSummary": "must not be stored" } }),
        ),
        ("addressed by name, stating none", true, json!({})),
        (
            "addressed by name, with deployment parameters",
            true,
            json!({ "DeploymentParameters": { "Tags": { "owner": "nobody" } } }),
        ),
    ] {
        let (application, addressed) = if by_name {
            (application_name.clone(), name.clone())
        } else {
            (scope.application.clone(), definition_id.clone())
        };
        // The sentence names the run and the definition exactly as the request addressed
        // them, so a row that addressed the definition by name reads back that name.
        let expected_message =
            format!("Experiment run 1 for experiment definition {addressed} is not running.");
        let refused = stop_run(&router, &application, &addressed, 1, &body).await;
        assert_refused(case, &refused, 400, "BadRequestException");
        assert_eq!(
            refused.string("Message").as_deref(),
            Some(expected_message.as_str()),
            "case {case:?}: expected the message {expected_message:?}, got {:?}",
            refused.string("Message")
        );

        let read = get_run(&router, &scope.application, &definition_id, 1).await;
        assert_eq!(
            read.json(),
            ended,
            "case {case:?}: a refused stop must write nothing, expected {ended}, got {}",
            read.json()
        );
        let events = list_events(&router, &scope.application, &definition_id, 1).await;
        assert_eq!(
            event_types(case, &events).len(),
            2,
            "case {case:?}: a refused stop records nothing on the timeline, got {}",
            events.json()
        );
    }
}

/// A stop ends the ONE run its path addresses and nothing else: the runs the definition
/// has already finished with keep the documents they finished with, a run of another
/// definition keeps running and leaves that definition reporting an experiment in flight,
/// and a stop aimed at a finished run never reaches the run that IS in flight.
///
/// The rows span the run number the stop addresses — the first run of a definition and the
/// ones after it, over a definition that keeps accumulating finished runs — which is what a
/// handler that ended "the running run" rather than the addressed one would confuse.
#[tokio::test]
async fn a_stop_ends_only_the_run_it_addresses() {
    const LABEL: &str = "addressed";
    let router = router();
    let scope = scope(&router, LABEL).await;

    // A second definition under the same application, with a run in flight for the whole
    // test: no stop aimed at the definition under test may touch either of them.
    let bystander_name = format!("{LABEL}-bystander");
    let (bystander_id, bystander_run) = running(
        &router,
        "the other definition's run",
        &scope,
        &bystander_name,
        &json!({ "ExposurePercentage": 25.0 }),
    )
    .await;

    let name = format!("{LABEL}-definition");
    let definition = create_test_definition(&router, &scope, &name).await;
    let definition_id = id_of(&name, &definition);

    // The runs this definition has finished with, and the document each of them ended as.
    let mut finished: Vec<(i64, Value)> = Vec::new();
    for run in [1_i64, 2, 3] {
        let case = format!(
            "run {run}, stopped with {} finished run(s) behind it",
            finished.len()
        );
        let started = start_run(
            &router,
            &scope.application,
            &definition_id,
            &json!({ "ExposurePercentage": 10.0 }),
        )
        .await;
        assert_eq!(
            started.status,
            201,
            "case {case:?}: the run under test must start, got {} / {}",
            started.status,
            String::from_utf8_lossy(&started.body)
        );
        assert_eq!(
            started.json().get("Run").and_then(Value::as_i64),
            Some(run),
            "case {case:?}: expected run {run}, got {}",
            started.json()
        );

        // A stop aimed at a run this definition has already finished with is refused, and
        // the run that IS in flight is not the one it ends.
        for (earlier, _) in &finished {
            let refused =
                stop_run(&router, &scope.application, &definition_id, *earlier, &json!({})).await;
            assert_refused(&case, &refused, 400, "BadRequestException");
            let in_flight = get_run(&router, &scope.application, &definition_id, run).await;
            assert_eq!(
                in_flight.json().get("Status").and_then(Value::as_str),
                Some(state::EXPERIMENT_RUN_RUNNING),
                "case {case:?}: a refused stop of run {earlier} must leave run {run} running, \
                 got {}",
                in_flight.json()
            );
        }

        let summary = format!("run {run} concluded");
        let stopped = stop_run(
            &router,
            &scope.application,
            &definition_id,
            run,
            &json!({ "Result": { "ExecutiveSummary": summary } }),
        )
        .await;
        assert_eq!(
            stopped.status,
            200,
            "case {case:?}: expected 200, got {} / {}",
            stopped.status,
            String::from_utf8_lossy(&stopped.body)
        );
        let ended = stopped.json();
        assert_eq!(
            ended.get("Run").and_then(Value::as_i64),
            Some(run),
            "case {case:?}: the stop reports the run it addressed, got {ended}"
        );
        assert_eq!(
            ended.get("Status").and_then(Value::as_str),
            Some(state::EXPERIMENT_RUN_DONE),
            "case {case:?}: expected the finished status, got {ended}"
        );
        assert_eq!(
            ended.get("Result"),
            Some(&json!({ "ExecutiveSummary": summary })),
            "case {case:?}: expected the conclusion this stop stated, got {ended}"
        );

        // Every run finished EARLIER still reports the document it finished with: this
        // stop wrote none of them.
        for (earlier, document) in &finished {
            let read = get_run(&router, &scope.application, &definition_id, *earlier).await;
            assert_eq!(
                read.status,
                200,
                "case {case:?}: run {earlier} must still read back, got {} / {}",
                read.status,
                String::from_utf8_lossy(&read.body)
            );
            assert_eq!(
                &read.json(),
                document,
                "case {case:?}: expected run {earlier} to still report {document}, got {}",
                read.json()
            );
        }
        finished.push((run, ended));

        // The other definition's run is in flight exactly as it was, and that definition
        // still reports the status it holds while an experiment runs.
        let other_run = get_run(&router, &scope.application, &bystander_id, 1).await;
        assert_eq!(
            other_run.status,
            200,
            "case {case:?}: the other definition's run must still read back, got {} / {}",
            other_run.status,
            String::from_utf8_lossy(&other_run.body)
        );
        let other = other_run.json();
        assert_eq!(
            other.get("Status").and_then(Value::as_str),
            Some(state::EXPERIMENT_RUN_RUNNING),
            "case {case:?}: expected the other definition's run to still be running, got {other}"
        );
        assert_eq!(
            other, bystander_run,
            "case {case:?}: expected the other definition's run to report {bystander_run}, got \
             {other}"
        );
        let other_definition = get_definition(&router, &scope.application, &bystander_id).await;
        assert_eq!(
            other_definition.string("Status").as_deref(),
            Some(state::EXPERIMENT_DEFINITION_ACTIVE),
            "case {case:?}: expected the other definition to still hold a run in flight, got {}",
            other_definition.json()
        );
    }

    // Three runs were issued, stopped and kept, each under its own number.
    assert_eq!(
        finished.iter().map(|(run, _)| *run).collect::<Vec<_>>(),
        vec![1_i64, 2, 3],
        "expected one finished record per run number, got {:?}",
        finished.iter().map(|(run, _)| *run).collect::<Vec<_>>()
    );
}

/// An identifier or a run number that addresses nothing is HTTP 404
/// `ResourceNotFoundException`, and the refusal writes nothing.
#[tokio::test]
async fn a_stop_that_resolves_nothing_refuses_and_changes_nothing() {
    const LABEL: &str = "misses";
    let router = router();
    let scope = scope(&router, LABEL).await;
    let name = format!("{LABEL}-definition");
    let (definition_id, running_run) = running(
        &router,
        "the run under test",
        &scope,
        &name,
        &json!({ "TreatmentOverrides": { "Inline": string_map([("entity-1", "t1")]) } }),
    )
    .await;
    // A second application, so the definition can be addressed under the wrong parent.
    let other_application = create_application(&router, &format!("{LABEL}-other")).await;

    // Rows differ along WHICH level of the path is unresolvable: the application, the
    // definition, the definition under another application, and a run number the
    // definition never issued — the next one, a far one and the 32-bit upper end of
    // `PositiveInteger`.
    for (case, application, addressed, run) in [
        (
            "an application identifier nothing answers to",
            "nosuchapp".to_string(),
            definition_id.clone(),
            1_i64,
        ),
        (
            "a definition identifier nothing answers to",
            scope.application.clone(),
            "nosuchdef".to_string(),
            1,
        ),
        (
            "the definition under another application",
            other_application.clone(),
            definition_id.clone(),
            1,
        ),
        (
            "the run number after the one that was issued",
            scope.application.clone(),
            definition_id.clone(),
            2,
        ),
        (
            "a far run number",
            scope.application.clone(),
            name.clone(),
            10_000,
        ),
        (
            "the upper end of PositiveInteger",
            scope.application.clone(),
            definition_id.clone(),
            i64::from(i32::MAX),
        ),
    ] {
        let refused = stop_run(
            &router,
            &application,
            &addressed,
            run,
            &json!({ "Result": { "ExecutiveSummary": "must not be stored" } }),
        )
        .await;
        assert_refused(case, &refused, 404, "ResourceNotFoundException");
        assert!(
            refused
                .string("Message")
                .is_some_and(|message| !message.is_empty()),
            "case {case:?}: the refusal must carry a Message, got {}",
            refused.json()
        );

        // The run the definition does hold is still running, exactly as it was.
        let read = get_run(&router, &scope.application, &definition_id, 1).await;
        assert_eq!(
            read.json(),
            running_run,
            "case {case:?}: a refused stop must write nothing, expected {running_run}, got {}",
            read.json()
        );
    }

    // And the overrides map is the one shape the start wrote, so the row above compared a
    // document that carries one.
    assert_eq!(
        running_run.get("TreatmentOverrides"),
        Some(&json!({ "Inline": string_map([("entity-1", "t1")]) })),
        "the run under test must carry the overrides the start assigned, got {running_run}"
    );
}
